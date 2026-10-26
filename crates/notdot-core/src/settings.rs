use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub hue: u16,
    pub voice_id: String,
    pub voice_provider: String,
    pub voice_name: String,
    pub memory_enabled: bool,
    pub onboarding_complete: bool,
    pub thread_id: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "notdot".into(),
            hue: 192,
            voice_id: String::new(),
            voice_provider: "elevenlabs".into(),
            voice_name: String::new(),
            memory_enabled: false,
            onboarding_complete: false,
            thread_id: None,
        }
    }
}
impl Settings {
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
        if self.name.chars().any(char::is_control)
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
        s.hue = 500;
        assert!(s.save(&p).is_err());
        assert_eq!(Settings::load(&p).unwrap().hue, 192);
    }
}
