//! UI Automation driver for ordinary, visible windows in this Windows session.
use super::{App, MAX_DEPTH, MAX_NODES, Node, Status};
use crate::clean;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
                IUIAutomationTreeWalker, UIA_ButtonControlTypeId, UIA_InvokePatternId,
            },
            WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible},
        },
    },
    core::{BOOL, Interface, PWSTR},
};

struct Apartment;
impl Apartment {
    fn new() -> Result<Self, String> {
        // SAFETY: one matching uninitialise occurs on this same thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(|e| e.to_string())?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this thread successfully initialized COM.
        unsafe { CoUninitialize() };
    }
}

fn automation() -> Result<IUIAutomation, String> {
    // SAFETY: callers hold an Apartment on this thread.
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| e.to_string())
}

pub(super) fn readiness() -> Status {
    // The UI thread may already use a single-threaded COM apartment.
    let available = std::thread::spawn(|| {
        Apartment::new()
            .and_then(|_apartment| automation().map(|_| ()))
            .is_ok()
    })
    .join()
    .unwrap_or(false);
    match available {
        true => Status::Ready,
        false => {
            Status::Unsupported("Windows UI Automation is unavailable in this session.".into())
        }
    }
}

fn windows() -> Vec<(u32, HWND)> {
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: EnumWindows is synchronous and the vector is live throughout.
        let list = unsafe { &mut *(data.0 as *mut Vec<(u32, HWND)>) };
        if list.len() >= 256 {
            return BOOL(0);
        }
        let mut pid = 0;
        // SAFETY: hwnd comes from EnumWindows and pid is writable.
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid != 0 {
                    list.push((pid, hwnd));
                }
            }
        }
        BOOL(1)
    }
    let mut list = Vec::new();
    // SAFETY: visit has the requested ABI and cannot outlive list.
    let _ = unsafe {
        EnumWindows(
            Some(visit),
            LPARAM((&mut list as *mut Vec<(u32, HWND)>) as isize),
        )
    };
    list
}

fn image_name(pid: u32) -> Option<String> {
    // SAFETY: only query access is requested and the returned handle is closed once.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = String::from_utf16(buffer.get(..length as usize)?).ok()?;
    let name = Path::new(&path).file_name()?.to_str()?;
    if name.len() > 100 || !name.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    Some(clean(name, 100))
}

pub(super) fn apps() -> Result<Vec<App>, String> {
    let mut pids = std::collections::HashSet::new();
    Ok(windows()
        .into_iter()
        .filter(|(pid, _)| *pid != std::process::id() && pids.insert(*pid))
        .filter_map(|(pid, _)| {
            let name = image_name(pid)?;
            Some(App {
                bundle_id: format!("win:{}:{pid}", name.to_ascii_lowercase()),
                name,
                pid: i32::try_from(pid).ok()?,
            })
        })
        .collect())
}

fn roots(uia: &IUIAutomation, pid: i32) -> Vec<(usize, IUIAutomationElement)> {
    let mut owned: Vec<_> = windows()
        .into_iter()
        .filter(|(owner, _)| i32::try_from(*owner).ok() == Some(pid))
        .map(|(_, handle)| handle)
        .collect();
    owned.sort_unstable_by_key(|handle| handle.0 as usize);
    owned
        .into_iter()
        .take(16)
        .enumerate()
        .filter_map(|(index, handle)| {
            // SAFETY: handle came from this session. The returned element's PID
            // is independently checked before reading it.
            let root = unsafe { uia.ElementFromHandle(handle) }.ok()?;
            (unsafe { root.CurrentProcessId() }.ok()? == pid).then_some((index, root))
        })
        .collect()
}

fn describe(element: &IUIAutomationElement, pid: i32, reference: &str) -> Option<Node> {
    // SAFETY: reads typed properties from a live COM element in this apartment.
    unsafe {
        if element.CurrentProcessId().ok()? != pid || element.CurrentIsPassword().ok()?.as_bool() {
            return None;
        }
        let control = element.CurrentControlType().ok()?;
        let disabled = !element.CurrentIsEnabled().ok()?.as_bool();
        let label = clean(&element.CurrentName().ok()?.to_string(), 120);
        let pressable = control == UIA_ButtonControlTypeId && !disabled;
        Some(Node {
            reference: reference.into(),
            role: format!("UIA{}", control.0),
            label,
            value: String::new(),
            pressable,
            disabled,
        })
    }
}

fn invoke(element: &IUIAutomationElement) -> Result<(), String> {
    // SAFETY: QueryInterface via cast verifies the pattern type before Invoke.
    let pattern: IUIAutomationInvokePattern =
        unsafe { element.GetCurrentPattern(UIA_InvokePatternId) }
            .and_then(|pattern| pattern.cast())
            .map_err(|_| "This button no longer supports Invoke".to_owned())?;
    unsafe { pattern.Invoke() }.map_err(|e| e.to_string())
}

struct Walk<'a> {
    walker: &'a IUIAutomationTreeWalker,
    pid: i32,
    deadline: Instant,
    nodes: Vec<Node>,
    target: Option<(&'a str, &'a str, &'a str)>,
    pressed: bool,
}

impl Walk<'_> {
    fn visit(
        &mut self,
        element: IUIAutomationElement,
        reference: String,
        depth: usize,
    ) -> Result<(), String> {
        if depth >= MAX_DEPTH
            || self.nodes.len() >= MAX_NODES
            || self.pressed
            || Instant::now() >= self.deadline
        {
            return Ok(());
        }
        let Some(node) = describe(&element, self.pid, &reference) else {
            return Ok(());
        };
        if let Some((wanted, role, label)) = self.target
            && wanted == reference
        {
            if node.role != role || node.label != label || !node.pressable || node.label.is_empty()
            {
                return Err("That button changed. Look at the app again.".into());
            }
            invoke(&element)?;
            self.pressed = true;
            return Ok(());
        }
        self.nodes.push(node);
        // SAFETY: the walker and elements belong to the apartment on this thread.
        let mut child = unsafe { self.walker.GetFirstChildElement(&element) };
        for index in 0..MAX_NODES {
            if self.nodes.len() >= MAX_NODES || self.pressed || Instant::now() >= self.deadline {
                break;
            }
            let Ok(element) = child else { break };
            self.visit(element.clone(), format!("{reference}.{index}"), depth + 1)?;
            child = unsafe { self.walker.GetNextSiblingElement(&element) };
        }
        Ok(())
    }
}

fn walk(pid: i32, target: Option<(&str, &str, &str)>) -> Result<Vec<Node>, String> {
    let _apartment = Apartment::new()?;
    let uia = automation()?;
    // SAFETY: the automation object is live in this apartment.
    let walker = unsafe { uia.ControlViewWalker() }.map_err(|e| e.to_string())?;
    let mut walk = Walk {
        walker: &walker,
        pid,
        deadline: Instant::now() + Duration::from_secs(8),
        nodes: Vec::new(),
        target,
        pressed: false,
    };
    for (index, root) in roots(&uia, pid) {
        walk.visit(root, index.to_string(), 0)?;
    }
    if target.is_some() && !walk.pressed {
        return Err("That button is no longer available. Look at the app again.".into());
    }
    Ok(walk.nodes)
}

pub(super) fn observe(pid: i32) -> Result<Vec<Node>, String> {
    walk(pid, None)
}

pub(super) fn press(pid: i32, reference: &str, role: &str, label: &str) -> Result<(), String> {
    walk(pid, Some((reference, role, label))).map(|_| ())
}
