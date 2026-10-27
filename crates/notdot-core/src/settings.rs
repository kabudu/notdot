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
    pub voice_name: String,
    pub memory_enabled: bool,
    pub initiative: crate::initiative::Policy,
    pub onboarding_complete: bool,
    pub thread_id: Option<String>,
    pub startup: Startup,
    pub open_desktop: bool,
    pub automatic_updates: bool,
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
            voice_name: String::new(),
            memory_enabled: false,
            initiative: crate::initiative::Policy::default(),
            onboarding_complete: false,
            thread_id: None,
            startup: Startup::Manual,
            open_desktop: false,
            automatic_updates: true,
            wake_enabled: false,
            wake_name_only: false,
            microphone_muted: false,
        }
    }
}
impl Settings {
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
        if !matches!(self.speech_language.as_str(), "en" | "auto")
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
        assert!(!legacy.wake_enabled);
        assert!(!legacy.wake_name_only);
        assert!(!legacy.microphone_muted);
        s.wake_enabled = true;
        s.wake_name_only = true;
        s.microphone_muted = true;
        s.save(&p).unwrap();
        let restored = Settings::load(&p).unwrap();
        assert!(restored.wake_enabled && restored.wake_name_only && restored.microphone_muted);

        s.hue = 500;
        assert!(s.save(&p).is_err());
        assert_eq!(Settings::load(&p).unwrap().hue, 192);
    }
}
