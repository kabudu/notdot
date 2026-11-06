//! Settings proposals cross the same native validation/persistence boundary as
//! the visible panel. The model never receives a write handle or a credential.
use super::{AppState, Settings};
use serde_json::{Value, json};
#[derive(serde::Deserialize)]
pub struct SaveGuard {
    pub expected: Value,
    pub request: String,
}
use std::sync::Arc;
use tauri::State;

fn wants_voice_choices(request: &str) -> bool {
    let lower = request.to_lowercase();
    let words: Vec<_> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().any(|word| matches!(*word, "voice" | "voices"))
        && words.iter().any(|word| {
            matches!(
                *word,
                "change"
                    | "choose"
                    | "switch"
                    | "available"
                    | "list"
                    | "which"
                    | "what"
                    | "different"
            )
        })
}
pub async fn context(state: &AppState, request: &str) -> Value {
    if !wants_voice_choices(request) {
        return Value::Null;
    }
    let settings = state.snapshot.lock().await.settings.clone();
    let mut result = json!({"voice_service":settings.voice_provider});
    if let Ok(provider) = super::backend(state, Some(settings.voice_provider)).await {
        match tokio::time::timeout(std::time::Duration::from_secs(3), provider.voices("", None))
            .await
        {
            Ok(Ok(page)) => {
                let more = page.voices.len() > 30 || page.next_cursor.is_some();
                result["voice_choices"] = json!(
                    page.voices
                        .into_iter()
                        .take(30)
                        .map(|voice| voice.name)
                        .collect::<Vec<_>>()
                );
                result["more_voices_in_settings"] = more.into();
            }
            _ => {
                result["voice_choices_status"] =
                    "Voice catalogue unavailable; do not invent available voices.".into()
            }
        }
    } else {
        result["voice_choices_status"] =
            "Allow or save the voice service key in Settings first.".into();
    }
    result
}

pub fn check_locked_save(
    state: &AppState,
    current: &Settings,
    expected: Option<&Value>,
    request: Option<&str>,
) -> Result<(), String> {
    let context = state.context();
    if let Some(base) = expected
        && *base != serde_json::to_value(current).expect("serializable settings")
    {
        return Err("Settings changed while you were deciding. Ask again so I can read back the current change.".into());
    }
    if request.is_some() {
        state.require_owner_voice()?;
    }
    if let Some(id) = request
        && !context
            .settings_consent
            .lock()
            .expect("settings consent")
            .issued_valid(id)
    {
        return Err("That settings change was cancelled or expired. Nothing was saved.".into());
    }
    Ok(())
}
pub async fn check_save(
    state: &AppState,
    expected: Option<&Value>,
    request: Option<&str>,
) -> Result<(), String> {
    check_locked_save(
        state,
        &state.snapshot.lock().await.settings,
        expected,
        request,
    )
}

async fn resolve_voice(
    state: &AppState,
    candidate: &mut Settings,
    patch: &notdot_core::voice_settings::Patch,
) -> Result<(), String> {
    if patch.voice_provider.is_none() && patch.voice_name.is_none() {
        return Ok(());
    }
    let name = patch
        .voice_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .ok_or(
            "Choose a voice name as well as the voice service. You can hear choices in Settings.",
        )?;
    let provider = super::backend(state, Some(candidate.voice_provider.clone())).await?;
    let mut cursor = None;
    let mut matches = Vec::new();
    for _ in 0..5 {
        let page = provider
            .voices(name, cursor.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        for voice in page.voices {
            if voice.name.trim().eq_ignore_ascii_case(name.trim()) {
                matches.push(voice);
            }
            if matches.len() > 1 {
                return Err(
                    "More than one voice has that name. Choose the exact voice in Settings.".into(),
                );
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    if cursor.is_some() {
        return Err("That voice search has too many results. Choose the voice in Settings.".into());
    }
    let voice = matches.pop().ok_or("That voice is not available from your selected service. Ask for available voices or choose one in Settings.")?;
    candidate.voice_id = voice.id;
    candidate.voice_name = voice.name;
    Ok(())
}

#[tauri::command]
pub async fn settings_question(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<String, String> {
    let context = state.context();
    super::session_ready(&window, &state).await?;
    context
        .settings_consent
        .lock()
        .expect("settings consent")
        .question(&id)
}
#[tauri::command]
pub fn cancel_settings(state: State<'_, Arc<AppState>>, id: Option<String>) {
    let context = state.context();
    let mut consent = context.settings_consent.lock().expect("settings consent");
    if let Some(id) = id {
        consent.cancel_id(&id);
    } else {
        consent.cancel();
    }
}
#[tauri::command]
pub async fn answer_settings(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    text: String,
    capture_token: Option<String>,
) -> Result<String, String> {
    crate::profile_access::check_capture(&state, capture_token.as_deref())?;
    state.require_owner_voice()?;
    let context = state.context();
    if text.len() > 24000 {
        return Err("Settings confirmation is too long".into());
    }
    super::session_ready(&window, &state).await?;
    let current = state.snapshot.lock().await.settings.clone();
    let approved = context
        .settings_consent
        .lock()
        .expect("settings consent")
        .answer(&id, &text, &current)?;
    let Some(approved) = approved else {
        return Ok("Cancelled. Your settings are unchanged.".into());
    };
    let result = async {
        let mut candidate = approved.patch().apply(approved.base())?;
        // Validate installed services and existing secure setup without requesting
        // background Keychain approval or accepting spoken credentials.
        if candidate.stt_provider != current.stt_provider
            && !state.credentials.cached(candidate.transcription_id())
        {
            return Err("Save or allow that speech-recognition key in Settings first.".into());
        }
        if candidate.voice_provider != current.voice_provider
            && !state.credentials.cached(&candidate.voice_provider)
        {
            return Err("Save or allow that voice-service key in Settings first.".into());
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            resolve_voice(&state, &mut candidate, approved.patch()),
        )
        .await
        .map_err(|_| "Voice search took too long; no settings were saved".to_string())??;
        if !approved.is_valid() {
            return Err("Settings change cancelled or expired".into());
        }
        let expected = serde_json::to_value(approved.base()).expect("serializable settings");
        super::save_settings(
            app,
            state.clone(),
            candidate,
            None,
            None,
            Some(approved.patch().model.is_some() || approved.patch().effort.is_some()),
            Some(SaveGuard {
                expected,
                request: approved.id().to_owned(),
            }),
        )
        .await
    }
    .await;
    context
        .settings_consent
        .lock()
        .expect("settings consent")
        .cancel_id(approved.id());
    match result {
        Ok(()) => Ok("Your settings are saved.".into()),
        Err(error) => {
            if let Some(detail) = error.strip_prefix("Settings saved; ") {
                Ok(format!(
                    "Your settings were saved, but notdot needs attention: {detail}"
                ))
            } else {
                Ok(format!("I could not change your settings: {error}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn voice_catalogue_lookup_does_not_delay_unrelated_invoice_requests() {
        for text in [
            "Change the invoice date",
            "What is this invoice?",
            "List my invoices",
            "Make the orb blue",
        ] {
            assert!(!super::wants_voice_choices(text));
        }
        for text in [
            "What voices are available?",
            "Change my voice to Bella",
            "Choose a different voice",
        ] {
            assert!(super::wants_voice_choices(text));
        }
    }
}
