//! The installed, authenticated runtime owns availability. Never persist a catalog.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub model: String,
    pub effort: String,
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            model: "gpt-6.1-terra".into(),
            effort: "medium".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effort {
    pub reasoning_effort: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub model: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub default_reasoning_effort: String,
    pub supported_reasoning_efforts: Vec<Effort>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub is_default: bool,
    pub input_modalities: Option<Vec<String>>,
}
impl Model {
    pub fn visible(&self) -> bool {
        !self.hidden
            && !self.model.is_empty()
            && !self.supported_reasoning_efforts.is_empty()
            && self
                .input_modalities
                .as_ref()
                .is_none_or(|m| m.iter().any(|m| m == "text"))
    }
}
pub fn validate(models: &[Model], selection: &Selection) -> Result<(), String> {
    if models.iter().any(|m| {
        m.visible()
            && m.model == selection.model
            && m.supported_reasoning_efforts
                .iter()
                .any(|e| e.reasoning_effort == selection.effort)
    }) {
        Ok(())
    } else {
        Err("Your selected model or effort is no longer available from this runtime/account. Open Settings and choose from the refreshed model list.".into())
    }
}
