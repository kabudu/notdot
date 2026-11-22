//! Desktop apps through the platform accessibility API.
//!
//! macOS uses the AX API, which needs the person to allow notdot under
//! System Settings › Privacy & Security › Accessibility. notdot only checks
//! that permission; it asks the system to show its prompt only when the person
//! presses notdot's own setup button ([`request_setup`]).
//!
//! Windows (UI Automation) and Linux (AT-SPI over D-Bus) are not implemented;
//! [`readiness`] reports them as unsupported rather than pretending.
use crate::Status;
use serde::Serialize;

/// Bounds on one observation, so a huge window cannot exhaust memory or time.
pub const MAX_NODES: usize = 300;
pub const MAX_DEPTH: usize = 14;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
    pub pid: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    /// Child-index path from the app's windows, for example "0.3.1".
    #[serde(rename = "ref")]
    pub reference: String,
    pub role: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pressable: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

pub fn readiness() -> Status {
    #[cfg(target_os = "macos")]
    {
        if macos::trusted() {
            Status::Ready
        } else {
            Status::SetupRequired(
                "Allow notdot under System Settings › Privacy & Security › Accessibility, then reconnect.".into(),
            )
        }
    }
    #[cfg(target_os = "windows")]
    {
        Status::Unsupported(
            "Desktop control on Windows (UI Automation) is not available yet.".into(),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Status::Unsupported("Desktop control on Linux (AT-SPI) is not available yet.".into())
    }
}

/// Ask the OS to show its Accessibility prompt. Call only from a setup button
/// the person pressed; it never grants anything by itself.
pub fn request_setup() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::request()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Running, top-level desktop apps (not helper processes).
pub fn apps() -> Result<Vec<App>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::apps()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Desktop control is not available on this system".into())
    }
}

/// Read the app's windows as a bounded accessibility outline.
pub fn observe(pid: i32) -> Result<Vec<Node>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::observe(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err("Desktop control is not available on this system".into())
    }
}

/// Press the element at `reference` if it still has the role and label the
/// person confirmed. Returns an error, never a guess, when it changed.
pub fn press(pid: i32, reference: &str, role: &str, label: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::press(pid, reference, role, label)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, reference, role, label);
        Err("Desktop control is not available on this system".into())
    }
}

/// The top-level app bundle of an executable path, or `None` for helpers
/// nested inside another bundle and for non-bundled processes.
pub fn bundle_root(executable: &str) -> Option<&str> {
    let marker = ".app/Contents/MacOS/";
    let at = executable.find(marker)?;
    let root = &executable[..at + 4];
    // A nested helper has an earlier ".app/" in its path.
    if root[..root.len() - 4].contains(".app/") {
        return None;
    }
    Some(root)
}

/// Visible, ordinary windows owned by the selected application.
pub fn screenshot_windows(pid: i32) -> Result<Vec<u32>, String> {
    #[cfg(target_os = "macos")]
    return macos::screenshot_windows(pid);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err("Window screenshots are unavailable on this system".into())
    }
}

pub async fn screenshot(pid: i32, window: u32) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    return macos::screenshot(pid, window).await;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, window);
        Err("Window screenshots are unavailable on this system".into())
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use super::{App, MAX_DEPTH, MAX_NODES, Node};
    use crate::clean;
    use core_foundation::{
        array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef},
        base::{CFGetTypeID, CFType, CFTypeRef, TCFType},
        boolean::CFBoolean,
        bundle::CFBundle,
        dictionary::{CFDictionary, CFDictionaryRef},
        number::CFNumber,
        string::{CFString, CFStringRef},
        url::CFURL,
    };
    use std::ffi::c_void;

    type AxRef = *const c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> AxRef;
        fn AXUIElementCopyAttributeValue(
            element: AxRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementPerformAction(element: AxRef, action: CFStringRef) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AxRef, seconds: f32) -> i32;
        static kAXTrustedCheckOptionPrompt: CFStringRef;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGWindowListCopyWindowInfo(options: u32, relative_to: u32) -> CFArrayRef;
    }

    pub(super) fn screenshot_windows(pid: i32) -> Result<Vec<u32>, String> {
        // SAFETY: these calls have no pointers supplied by the caller.
        let list = unsafe {
            if !CGPreflightScreenCaptureAccess() {
                return Err(
                    "Window capture requires macOS Screen Recording permission for notdot.".into(),
                );
            }
            let list = CGWindowListCopyWindowInfo(1 | 16, 0);
            if list.is_null() {
                return Err("macOS could not list visible windows.".into());
            }
            core_foundation::array::CFArray::<CFDictionary<CFString, CFType>>::wrap_under_create_rule(list)
        };
        let number = |window: &CFDictionary<CFString, CFType>, key: &str| {
            window
                .find(CFString::new(key))?
                .downcast::<CFNumber>()?
                .to_i64()
        };
        Ok(list
            .iter()
            .take(1024)
            .filter_map(|window| {
                (number(&window, "kCGWindowOwnerPID")? == i64::from(pid)
                    && number(&window, "kCGWindowLayer")? == 0)
                    .then(|| u32::try_from(number(&window, "kCGWindowNumber")?).ok())
                    .flatten()
            })
            .take(16)
            .collect())
    }

    pub(super) async fn screenshot(pid: i32, window: u32) -> Result<Vec<u8>, String> {
        if !screenshot_windows(pid)?.contains(&window) {
            return Err("The selected window is no longer visible in that app.".into());
        }
        let temp =
            tempfile::tempdir().map_err(|e| format!("Could not prepare window capture: {e}"))?;
        let file = temp.path().join("window.jpg");
        let mut command = tokio::process::Command::new("/usr/sbin/screencapture");
        command
            .args(["-x", "-o", "-t", "jpg"])
            .arg(format!("-l{window}"))
            .arg(&file)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let status = tokio::time::timeout(std::time::Duration::from_secs(8), command.status())
            .await
            .map_err(|_| "Window capture timed out")?
            .map_err(|e| format!("Window capture failed: {e}"))?;
        if !status.success() || !screenshot_windows(pid)?.contains(&window) {
            return Err("The selected window could not be captured.".into());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(file)
            .map_err(|_| "Window capture produced no image")?
            .take(1_125_001)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read the captured image")?;
        if bytes.len() > 1_125_000 || !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            return Err("The window image exceeded the capture limit or was invalid.".into());
        }
        Ok(bytes)
    }

    pub(super) fn trusted() -> bool {
        // SAFETY: no arguments; reads this process's TCC state.
        unsafe { AXIsProcessTrusted() }
    }

    pub(super) fn request() -> bool {
        // SAFETY: the key is a framework constant; the dictionary outlives the call.
        unsafe {
            let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
            let options = CFDictionary::from_CFType_pairs(&[(
                key.as_CFType(),
                CFBoolean::true_value().as_CFType(),
            )]);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
        }
    }

    pub(super) fn apps() -> Result<Vec<App>, String> {
        let mut pids = vec![0i32; 4096];
        // SAFETY: the buffer and its byte size describe the same allocation.
        let count = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast(),
                (pids.len() * std::mem::size_of::<i32>()) as i32,
            )
        };
        if count <= 0 {
            return Err("Could not list running apps".into());
        }
        pids.truncate(count as usize);
        let mut found: Vec<App> = Vec::new();
        for pid in pids {
            let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            // SAFETY: the buffer length matches the size passed.
            let len =
                unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            if len <= 0 {
                continue;
            }
            path.truncate(len as usize);
            let Ok(path) = String::from_utf8(path) else {
                continue;
            };
            let Some(root) = super::bundle_root(&path) else {
                continue;
            };
            let Some(url) = CFURL::from_path(root, true) else {
                continue;
            };
            let Some(bundle) = CFBundle::new(url) else {
                continue;
            };
            let info = bundle.info_dictionary();
            let text = |key: &str| {
                info.find(CFString::new(key))
                    .and_then(|v| v.downcast::<CFString>())
                    .map(|s| s.to_string())
            };
            let Some(bundle_id) = text("CFBundleIdentifier") else {
                continue;
            };
            // Background agents and UI-less helpers are not desktop apps.
            let flag = |key: &str| {
                info.find(CFString::new(key)).is_some_and(|v| {
                    v.downcast::<CFBoolean>().is_some_and(bool::from)
                        || v.downcast::<CFString>().is_some_and(|s| s == "1")
                })
            };
            if flag("LSBackgroundOnly") || flag("LSUIElement") {
                continue;
            }
            let name = text("CFBundleDisplayName")
                .or_else(|| text("CFBundleName"))
                .unwrap_or_else(|| bundle_id.clone());
            if !found.iter().any(|a| a.bundle_id == bundle_id) {
                found.push(App {
                    name: clean(&name, 80),
                    bundle_id,
                    pid,
                });
            }
        }
        found.sort_by_key(|a| a.name.to_lowercase());
        Ok(found)
    }

    /// An owned AXUIElement.
    struct Element(CFType);
    impl Element {
        fn application(pid: i32) -> Result<Self, String> {
            // SAFETY: Create rule: we own the returned reference.
            let raw = unsafe { AXUIElementCreateApplication(pid) };
            if raw.is_null() {
                return Err("That app is not running".into());
            }
            // SAFETY: raw is a valid, owned CF object.
            let element = Self(unsafe { CFType::wrap_under_create_rule(raw) });
            // Bound every later call to this app (and its children) to 2 s.
            // SAFETY: valid element.
            unsafe { AXUIElementSetMessagingTimeout(element.raw(), 2.0) };
            Ok(element)
        }
        fn raw(&self) -> AxRef {
            self.0.as_CFTypeRef()
        }
        fn attribute(&self, name: &str) -> Option<CFType> {
            let key = CFString::new(name);
            let mut value: CFTypeRef = std::ptr::null();
            // SAFETY: valid element and key; Copy rule: we own `value` on success.
            let status = unsafe {
                AXUIElementCopyAttributeValue(self.raw(), key.as_concrete_TypeRef(), &mut value)
            };
            if status != 0 || value.is_null() {
                return None;
            }
            // SAFETY: owned reference returned by a Copy function.
            Some(unsafe { CFType::wrap_under_create_rule(value) })
        }
        fn text(&self, name: &str) -> String {
            self.attribute(name)
                .and_then(|v| {
                    v.downcast::<CFString>().map(|s| s.to_string()).or_else(|| {
                        v.downcast::<CFNumber>()
                            .and_then(|n| n.to_f64())
                            .map(|n| n.to_string())
                    })
                })
                .unwrap_or_default()
        }
        fn flag(&self, name: &str) -> Option<bool> {
            self.attribute(name)
                .and_then(|v| v.downcast::<CFBoolean>())
                .map(bool::from)
        }
        fn list(&self, name: &str) -> Vec<Element> {
            let Some(value) = self.attribute(name) else {
                return Vec::new();
            };
            let raw = value.as_CFTypeRef();
            // SAFETY: type checked before use as an array; items are borrowed
            // (Get rule) and retained by wrap_under_get_rule.
            unsafe {
                if CFGetTypeID(raw) != CFArrayGetTypeID() {
                    return Vec::new();
                }
                let array = raw as CFArrayRef;
                (0..CFArrayGetCount(array).min(512))
                    .filter_map(|i| {
                        let item = CFArrayGetValueAtIndex(array, i);
                        (!item.is_null()).then(|| Element(CFType::wrap_under_get_rule(item)))
                    })
                    .collect()
            }
        }
        fn actions(&self) -> Vec<String> {
            let mut names: CFTypeRef = std::ptr::null();
            #[link(name = "ApplicationServices", kind = "framework")]
            unsafe extern "C" {
                fn AXUIElementCopyActionNames(element: AxRef, names: *mut CFTypeRef) -> i32;
            }
            // SAFETY: valid element; Copy rule on success.
            let status = unsafe { AXUIElementCopyActionNames(self.raw(), &mut names) };
            if status != 0 || names.is_null() {
                return Vec::new();
            }
            // SAFETY: owned reference returned by a Copy function.
            let names = unsafe { CFType::wrap_under_create_rule(names) };
            let raw = names.as_CFTypeRef();
            // SAFETY: as in `list`.
            unsafe {
                if CFGetTypeID(raw) != CFArrayGetTypeID() {
                    return Vec::new();
                }
                let array = raw as CFArrayRef;
                (0..CFArrayGetCount(array).min(32))
                    .filter_map(|i| {
                        let item = CFArrayGetValueAtIndex(array, i);
                        (!item.is_null())
                            .then(|| CFType::wrap_under_get_rule(item))
                            .and_then(|v| v.downcast::<CFString>())
                            .map(|s| s.to_string())
                    })
                    .collect()
            }
        }
        fn describe(&self) -> (String, String, String) {
            let role = self.text("AXRole");
            let label = [self.text("AXTitle"), self.text("AXDescription")]
                .into_iter()
                .find(|s| !s.trim().is_empty())
                .unwrap_or_default();
            // Never read secure text fields' values.
            let value =
                if role == "AXSecureTextField" || self.text("AXSubrole") == "AXSecureTextField" {
                    String::new()
                } else {
                    self.text("AXValue")
                };
            (clean(&role, 40), clean(&label, 120), clean(&value, 200))
        }
    }

    fn windows(pid: i32) -> Result<Vec<Element>, String> {
        if !trusted() {
            return Err("notdot is not allowed to use Accessibility yet".into());
        }
        let app = Element::application(pid)?;
        let windows = app.list("AXWindows");
        if windows.is_empty() {
            return Err("That app has no open window notdot can read".into());
        }
        Ok(windows.into_iter().take(8).collect())
    }

    pub(super) fn observe(pid: i32) -> Result<Vec<Node>, String> {
        let mut out = Vec::new();
        let mut stack: Vec<(Element, String, usize)> = windows(pid)?
            .into_iter()
            .enumerate()
            .map(|(i, w)| (w, i.to_string(), 0))
            .rev()
            .collect();
        while let Some((element, path, depth)) = stack.pop() {
            if out.len() >= MAX_NODES {
                break;
            }
            let (role, label, value) = element.describe();
            let pressable = element.actions().iter().any(|a| a == "AXPress");
            // Skip anonymous layout groups, but still walk into them.
            if pressable || !label.is_empty() || !value.is_empty() || depth == 0 {
                out.push(Node {
                    reference: path.clone(),
                    role,
                    label,
                    value,
                    pressable,
                    disabled: element.flag("AXEnabled") == Some(false),
                });
            }
            if depth < MAX_DEPTH {
                let children = element.list("AXChildren");
                for (i, child) in children.into_iter().enumerate().rev() {
                    stack.push((child, format!("{path}.{i}"), depth + 1));
                }
            }
        }
        Ok(out)
    }

    fn resolve(pid: i32, reference: &str) -> Result<Element, String> {
        let mut parts = reference.split('.').map(|p| p.parse::<usize>());
        let first = parts
            .next()
            .and_then(Result::ok)
            .ok_or("Unknown element reference")?;
        let mut element = windows(pid)?
            .into_iter()
            .nth(first)
            .ok_or("That window is gone. Look at the app again.")?;
        for part in parts {
            let index = part.map_err(|_| "Unknown element reference")?;
            element = element
                .list("AXChildren")
                .into_iter()
                .nth(index)
                .ok_or("That element is gone. Look at the app again.")?;
        }
        Ok(element)
    }

    pub(super) fn press(pid: i32, reference: &str, role: &str, label: &str) -> Result<(), String> {
        if reference.split('.').count() > MAX_DEPTH + 1 {
            return Err("Unknown element reference".into());
        }
        let element = resolve(pid, reference)?;
        let (now_role, now_label, _) = element.describe();
        if now_role != role || now_label != label {
            return Err("That element changed. Look at the app again.".into());
        }
        if element.flag("AXEnabled") == Some(false) {
            return Err("That control is disabled".into());
        }
        let action = CFString::new("AXPress");
        // SAFETY: valid element and action string.
        let status =
            unsafe { AXUIElementPerformAction(element.raw(), action.as_concrete_TypeRef()) };
        if status != 0 {
            return Err(format!(
                "The app did not accept the press (AX error {status})"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_top_level_bundles_are_apps() {
        assert_eq!(
            bundle_root("/Applications/Counter.app/Contents/MacOS/Counter"),
            Some("/Applications/Counter.app")
        );
        assert_eq!(
            bundle_root(
                "/Applications/Big.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper"
            ),
            None
        );
        assert_eq!(bundle_root("/usr/bin/true"), None);
    }
}
