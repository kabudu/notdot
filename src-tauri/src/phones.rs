//! Phones: the paired-phone link running inside notdot.
//!
//! Off by default and owner-only. When on, notdot listens for its paired
//! phones over an end-to-end encrypted Iroh connection (same network only
//! until a relay is deployed). Phones act only on their own person's notes
//! and memories, through the same stores and permission checks as voice
//! turns; they never talk to Codex and never see provider keys. Sensitive
//! actions need an attested step-up key, the owner's grant here, and a
//! biometric on the phone for every action.
//!
//! Built only with the `phones` cargo feature: the link brings new
//! third-party packages whose notices are not yet in the shipped inventory.
//! Without it the commands report "not available in this build".
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(not(feature = "phones"))]
use serde_json::json;
use std::sync::Arc;
use tauri::State;

#[cfg(feature = "phones")]
mod runtime;
#[cfg(feature = "phones")]
pub mod status_list;
#[cfg(feature = "phones")]
pub use runtime::Phones;

/// Owner-chosen settings for phones, saved in the app's data folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PhoneSettings {
    pub enabled: bool,
    /// Android apps allowed to hold step-up keys.
    pub android_apps: Vec<AndroidAppSetting>,
    /// iPhone apps allowed, by App ID.
    pub apple_apps: Vec<AppleAppSetting>,
    /// App Attest launch validation categories accepted.
    pub apple_categories: Vec<u32>,
    /// Accept App Attest keys from development (debuggable) builds.
    pub apple_development: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidAppSetting {
    pub package: String,
    /// SHA-256 of each signing certificate, as 64 hex digits.
    pub signers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppleAppSetting {
    pub team_id: String,
    pub bundle_id: String,
}

/// How long one grant of sensitive actions lasts.
#[cfg(feature = "phones")]
pub const GRANT_MINUTES: u64 = 15;

fn valid_package(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    name.len() <= 255
        && parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

impl PhoneSettings {
    /// Check owner input before it is saved or used.
    pub fn validate(&self) -> Result<(), String> {
        if self.android_apps.len() > 1
            || self.apple_apps.len() > 1
            || self.apple_categories.len() > 8
        {
            return Err("This prototype accepts one Android app and one iPhone app".into());
        }
        for app in &self.android_apps {
            if !valid_package(&app.package) {
                return Err(format!("“{}” isn't an Android package name", app.package));
            }
            if app.signers.is_empty() || app.signers.len() > 4 {
                return Err("Each Android app needs one to four signing certificates".into());
            }
            if app
                .signers
                .iter()
                .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("A signing certificate fingerprint is 64 hex digits (SHA-256)".into());
            }
        }
        for app in &self.apple_apps {
            if app.team_id.len() != 10
                || !app
                    .team_id
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            {
                return Err("An Apple team ID is 10 capital letters and digits".into());
            }
            if !valid_package(&app.bundle_id.replace('-', "_")) {
                return Err(format!("“{}” isn't a bundle ID", app.bundle_id));
            }
        }
        if !self.apple_apps.is_empty() && self.apple_categories.is_empty() {
            return Err("Choose which App Attest category to accept for iPhones".into());
        }
        if self.apple_categories.iter().any(|c| *c > 1000) {
            return Err("Unknown App Attest category".into());
        }
        Ok(())
    }
}

/// The settings window's identity checks: the owner, using notdot on this
/// computer, with the owner's own profile active. Members, and voices,
/// never manage phones.
fn owner_here(state: &crate::AppState) -> Result<(), String> {
    let roster = state.speakers.roster()?;
    if roster.recovery_required() {
        return Err("Voice recognition needs attention on this computer first".into());
    }
    roster
        .authorise(
            &notdot_core::profiles::Actor::LocalDevice,
            notdot_core::profiles::Capability::ManageMembers,
        )
        .map_err(|e| e.to_string())?;
    if state.context().id != *roster.owner() {
        return Err("Choose Use my profile in People to manage phones".into());
    }
    Ok(())
}

#[cfg(not(feature = "phones"))]
pub struct Phones;
#[cfg(not(feature = "phones"))]
impl Phones {
    pub fn open(_: &std::path::Path) -> Result<Self, String> {
        Ok(Self)
    }
    pub async fn start_if_enabled(&self, _: &Arc<crate::AppState>) {}
    pub async fn stop(&self) {}
}

#[cfg(not(feature = "phones"))]
const UNAVAILABLE: &str = "Phones aren't available in this build of notdot";

#[tauri::command]
pub async fn phones_status(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    return Ok(state.phones.status(&state).await);
    #[cfg(not(feature = "phones"))]
    Ok(json!({"available": false, "reason": UNAVAILABLE}))
}

#[tauri::command]
pub async fn phones_set_enabled(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
    enabled: bool,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    {
        state.phones.set_enabled(state.inner(), enabled).await?;
        Ok(state.phones.status(&state).await)
    }
    #[cfg(not(feature = "phones"))]
    {
        let _ = enabled;
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
pub async fn phones_create_offer(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
    profile: String,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    return state.phones.create_offer(&state, &profile).await;
    #[cfg(not(feature = "phones"))]
    {
        let _ = profile;
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
pub async fn phones_decide(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
    offer: String,
    allow: bool,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    {
        state.phones.decide(&offer, allow).await?;
        Ok(state.phones.status(&state).await)
    }
    #[cfg(not(feature = "phones"))]
    {
        let _ = (offer, allow);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
pub async fn phones_remove(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
    device: String,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    {
        state.phones.remove(&device).await?;
        Ok(state.phones.status(&state).await)
    }
    #[cfg(not(feature = "phones"))]
    {
        let _ = device;
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
pub async fn phones_sensitive(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
    device: String,
    allow: bool,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    {
        state.phones.sensitive(&device, allow).await?;
        Ok(state.phones.status(&state).await)
    }
    #[cfg(not(feature = "phones"))]
    {
        let _ = (device, allow);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
pub async fn phones_set_settings(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
    settings: PhoneSettings,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    settings.validate()?;
    #[cfg(feature = "phones")]
    {
        state.phones.set_settings(state.inner(), settings).await?;
        Ok(state.phones.status(&state).await)
    }
    #[cfg(not(feature = "phones"))]
    Err(UNAVAILABLE.into())
}

#[tauri::command]
pub async fn phones_refresh_revocations(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<crate::AppState>>,
) -> Result<Value, String> {
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err("Open Settings on this computer to manage phones".into());
    }
    owner_here(&state)?;
    #[cfg(feature = "phones")]
    {
        state.phones.refresh_now().await;
        Ok(state.phones.status(&state).await)
    }
    #[cfg(not(feature = "phones"))]
    Err(UNAVAILABLE.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_input_is_checked_before_use() {
        let good = PhoneSettings {
            enabled: true,
            android_apps: vec![AndroidAppSetting {
                package: "dev.example.app".into(),
                signers: vec!["ab".repeat(32)],
            }],
            apple_apps: vec![AppleAppSetting {
                team_id: "ABCDE12345".into(),
                bundle_id: "dev.example.app".into(),
            }],
            apple_categories: vec![1],
            apple_development: false,
        };
        assert!(good.validate().is_ok());
        let bad = |change: &dyn Fn(&mut PhoneSettings)| {
            let mut s = good.clone();
            change(&mut s);
            s.validate().is_err()
        };
        assert!(bad(&|s| s.android_apps[0].package = "nodots".into()));
        assert!(bad(&|s| s.android_apps[0].package = "dev..x".into()));
        assert!(bad(&|s| s.android_apps[0].package = "dev.1x".into()));
        assert!(bad(&|s| s.android_apps[0].signers = vec!["zz".repeat(32)]));
        assert!(bad(&|s| s.android_apps[0].signers.clear()));
        assert!(bad(&|s| s.apple_apps[0].team_id = "abcde12345".into()));
        assert!(bad(&|s| s.apple_apps[0].bundle_id = "x".into()));
        assert!(bad(&|s| s.apple_categories.clear()));
        assert!(bad(&|s| s.android_apps = vec![s.android_apps[0].clone(); 9]));
        assert!(PhoneSettings::default().validate().is_ok());
    }
}
