use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

// Unknown fields in earlier settings files are ignored; existing shortcuts and
// the local ASR location remain readable without migrating user data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub stt_language: String,
    pub microphone_source: String,
    pub local_model_dir: String,
    pub shortcut_record: String,
    pub shortcut_cancel: String,
    pub shortcut_finish: String,
    pub notify_sound: bool,
    pub start_sound: String,
    pub setup_complete: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stt_language: "auto".into(),
            microphone_source: "system".into(),
            local_model_dir: std::env::var("QUIETTYPE_LOCAL_MODELS_DIR").unwrap_or_default(),
            shortcut_record: "Fn".into(),
            shortcut_cancel: "Esc".into(),
            shortcut_finish: "↵".into(),
            notify_sound: true,
            start_sound: "soft".into(),
            setup_complete: false,
        }
    }
}

pub struct SettingsStore {
    dir: PathBuf,
    pub update_lock: Mutex<()>,
    pub settings: RwLock<Settings>,
}

impl SettingsStore {
    pub fn new() -> Result<Self, String> {
        let dir = dirs::config_dir()
            .ok_or("Cannot determine config directory")?
            .join("com.ai-quick-input.assistant");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("settings.json");
        let settings = if path.exists() {
            serde_json::from_str(&fs::read_to_string(path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("无法读取设置，请检查 settings.json：{e}"))?
        } else {
            Settings::default()
        };
        Ok(Self {
            dir,
            update_lock: Mutex::new(()),
            settings: RwLock::new(settings),
        })
    }

    pub fn save_settings(&self) -> Result<(), String> {
        let settings = self.settings.read().map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(&*settings).map_err(|e| e.to_string())?;
        let temporary = self.dir.join("settings.json.tmp");
        fs::write(&temporary, json).map_err(|e| e.to_string())?;
        fs::rename(temporary, self.dir.join("settings.json")).map_err(|e| e.to_string())
    }
}

pub fn shortcut_to_tauri(value: &str) -> Result<String, String> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for token in value.split_whitespace() {
        match token {
            "⌘" => modifiers.push("Super"),
            "⌃" => modifiers.push("Control"),
            "⌥" => modifiers.push("Alt"),
            "⇧" => modifiers.push("Shift"),
            "Esc" => key = Some("Escape"),
            "↵" => key = Some("Enter"),
            other => key = Some(other),
        }
    }
    modifiers.push(key.ok_or("快捷键缺少主按键")?);
    Ok(modifiers.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_legacy_settings_without_enabling_removed_features() {
        let old = r#"{"sttMode":"cloud","llmEnabled":true,"lipEnabled":true,"showPet":true,"shortcutRecord":"⌥ ⇧ R","localModelDir":"/models","notifySound":false}"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.shortcut_record, "⌥ ⇧ R");
        assert_eq!(settings.local_model_dir, "/models");
        assert!(!settings.notify_sound);
        assert_eq!(settings.start_sound, "soft");
        assert_eq!(settings.microphone_source, "system");
        let saved = serde_json::to_value(&settings).unwrap();
        assert!(!settings.setup_complete);
        assert_eq!(saved.as_object().unwrap().len(), 9);
        assert!(saved.get("llmEnabled").is_none());
    }
    #[test]
    fn converts_display_shortcuts_to_tauri_format() {
        assert_eq!(shortcut_to_tauri("⌥ R").unwrap(), "Alt+R");
        assert_eq!(shortcut_to_tauri("⌘ ⇧ K").unwrap(), "Super+Shift+K");
        assert_eq!(shortcut_to_tauri("Esc").unwrap(), "Escape");
        assert_eq!(shortcut_to_tauri("↵").unwrap(), "Enter");
    }
}
