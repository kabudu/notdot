use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Startup {
    #[default]
    Manual,
    Login,
    WithCodex,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub model: String,
    pub effort: String,
    pub hue: u16,
    pub voice_id: String,
    pub voice_provider: String,
    pub speech_language: String,
    pub stt_provider: String,
    pub voice_name: String,
    pub memory_enabled: bool,
    pub session_control: crate::session_control::Mode,
    pub initiative: crate::initiative::Policy,
    pub onboarding_complete: bool,
    pub thread_id: Option<String>,
    pub startup: Startup,
    pub open_desktop: bool,
    pub automatic_updates: bool,
    pub followup_enabled: bool,
    pub followup_seconds: u32,
    pub wake_enabled: bool,
    pub wake_name_only: bool,
    pub microphone_muted: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "notdot".into(),
            model: crate::models::Selection::default().model,
            effort: crate::models::Selection::default().effort,
            hue: 192,
            voice_id: String::new(),
            voice_provider: "elevenlabs".into(),
            speech_language: "en".into(),
            stt_provider: "voice".into(),
            voice_name: String::new(),
            memory_enabled: false,
            session_control: crate::session_control::Mode::ReadOnly,
            initiative: crate::initiative::Policy::default(),
            onboarding_complete: false,
            thread_id: None,
            startup: Startup::Manual,
            open_desktop: false,
            automatic_updates: true,
            followup_enabled: false,
            followup_seconds: 8,
            wake_enabled: false,
            wake_name_only: false,
            microphone_muted: false,
        }
    }
}
impl Settings {
    pub fn transcription_id(&self) -> &str {
        if self.stt_provider == "voice" {
            &self.voice_provider
        } else {
            &self.stt_provider
        }
    }
    pub fn selection(&self) -> crate::models::Selection {
        crate::models::Selection {
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut settings: Self =
                    serde_json::from_slice(&bytes).map_err(|e| format!("Read settings: {e}"))?;
                if settings.voice_id.is_empty() {
                    settings.voice_name.clear();
                }
                Ok(settings)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Read settings: {e}")),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if !(3..=20).contains(&self.followup_seconds) {
            return Err("Choose a follow-up window between 3 and 20 seconds".into());
        }
        if !matches!(self.stt_provider.as_str(), "voice" | "deepgram")
            || (self.stt_provider == "deepgram" && self.speech_language != "en")
            || !matches!(self.speech_language.as_str(), "en" | "auto")
            || self.model.is_empty()
            || self.model.len() > 200
            || self.effort.is_empty()
            || self.effort.len() > 32
            || !self.initiative.valid()
            || self.name.chars().any(char::is_control)
            || self.name.trim().is_empty()
            || self.name.len() > 80
            || self.hue > 360
            || self.voice_id.len() > 128
            || self.voice_provider.len() > 64
            || self.voice_name.len() > 512
        {
            return Err("Invalid name, hue or voice".into());
        }
        let dir = path.parent().ok_or("Settings directory missing")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let temp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        use std::io::Write;
        let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_control_defaults_to_read_only_and_all_modes_persist() {
        use crate::session_control::Mode;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(
            Settings::load(&path).unwrap().session_control,
            Mode::ReadOnly
        );
        for mode in [Mode::ReadOnly, Mode::Confirm, Mode::WhenAsked] {
            let s = Settings {
                session_control: mode,
                ..Settings::default()
            };
            s.save(&path).unwrap();
            assert_eq!(Settings::load(&path).unwrap().session_control, mode);
        }
        std::fs::write(&path, r#"{"session_control":"allow_everything"}"#).unwrap();
        assert!(Settings::load(&path).is_err());
    }
    #[test]
    fn settings_survive_restart_and_reject_invalid() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut s = Settings {
            name: "Aster".into(),
            ..Settings::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p).unwrap().name, "Aster");
        s.startup = Startup::WithCodex;
        s.open_desktop = true;
        s.automatic_updates = false;
        s.save(&p).unwrap();
        let restored = Settings::load(&p).unwrap();
        assert_eq!(restored.startup, Startup::WithCodex);
        assert!(restored.open_desktop);
        assert!(!restored.automatic_updates);
        for mode in [Startup::Login, Startup::Manual] {
            s.startup = mode;
            s.save(&p).unwrap();
            assert_eq!(Settings::load(&p).unwrap().startup, mode);
        }
        let legacy: Settings = serde_json::from_str(r#"{"name":"Legacy"}"#).unwrap();
        assert_eq!(legacy.startup, Startup::Manual);
        assert_eq!(legacy.stt_provider, "voice");
        assert_eq!(legacy.transcription_id(), legacy.voice_provider);
        assert_eq!(legacy.selection(), crate::models::Selection::default());
        s.model = "runtime-model".into();
        s.effort = "high".into();
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p).unwrap().selection(), s.selection());
        assert_eq!(
            legacy.initiative.behaviour,
            crate::initiative::Behaviour::Reactive
        );
        assert!(legacy.initiative.valid());
        assert!(legacy.automatic_updates);
        assert!(!legacy.followup_enabled);
        assert_eq!(legacy.followup_seconds, 8);
        s.followup_enabled = true;
        s.followup_seconds = 12;
        s.save(&p).unwrap();
        let restored = Settings::load(&p).unwrap();
        assert!(restored.followup_enabled);
        assert_eq!(restored.followup_seconds, 12);
        for invalid in [0, 2, 21, u32::MAX] {
            s.followup_seconds = invalid;
            assert!(s.save(&p).is_err());
            assert_eq!(Settings::load(&p).unwrap().followup_seconds, 12);
        }
        s.followup_seconds = 8;
        assert!(!legacy.wake_enabled);
        assert!(!legacy.wake_name_only);
        assert!(!legacy.microphone_muted);
        s.wake_enabled = true;
        s.wake_name_only = true;
        s.microphone_muted = true;
        s.save(&p).unwrap();
        let restored = Settings::load(&p).unwrap();
        assert!(restored.wake_enabled && restored.wake_name_only && restored.microphone_muted);

        s.stt_provider = "deepgram".into();
        s.speech_language = "en".into();
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p).unwrap().transcription_id(), "deepgram");
        s.speech_language = "auto".into();
        assert!(s.save(&p).is_err());
        assert_eq!(Settings::load(&p).unwrap().speech_language, "en");
        s.stt_provider = "voice".into();
        s.speech_language = "en".into();
        s.hue = 500;
        assert!(s.save(&p).is_err());
        assert_eq!(Settings::load(&p).unwrap().hue, 192);
    }
}
