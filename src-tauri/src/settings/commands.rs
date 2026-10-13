use super::store::{Settings, SettingsStore};
use tauri::{Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

pub fn get_shortcut_registration_status(
    app: tauri::AppHandle,
    state: State<'_, SettingsStore>,
) -> Result<bool, String> {
    let settings = state.settings.read().map_err(|e| e.to_string())?;
    if settings.shortcut_record == "Fn" {
        return crate::native::util_helper::check_accessibility_permission();
    }
    let voice = super::store::shortcut_to_tauri(&settings.shortcut_record)?;
    Ok(app.global_shortcut().is_registered(voice.as_str()))
}

pub fn get_settings(state: State<'_, SettingsStore>) -> Result<Settings, String> {
    Ok(state.settings.read().map_err(|e| e.to_string())?.clone())
}

pub fn update_settings(
    app: tauri::AppHandle,
    state: State<'_, SettingsStore>,
    patch: serde_json::Value,
) -> Result<(), String> {
    let _update = state.update_lock.lock().map_err(|e| e.to_string())?;
    let previous = state.settings.read().map_err(|e| e.to_string())?.clone();
    let mut merged = serde_json::to_value(&previous).map_err(|e| e.to_string())?;
    let changes = patch.as_object().ok_or("设置必须是对象")?;
    for (key, value) in changes {
        if ![
            "sttLanguage",
            "microphoneSource",
            "shortcutRecord",
            "notifySound",
            "startSound",
            "setupComplete",
        ]
        .contains(&key.as_str())
        {
            return Err("不支持的设置".into());
        }
        merged
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), value.clone());
    }
    let next: Settings = serde_json::from_value(merged).map_err(|e| e.to_string())?;
    if !["system", "builtIn", "headset"].contains(&next.microphone_source.as_str()) {
        return Err("不支持的麦克风来源".into());
    }
    if next.microphone_source != previous.microphone_source {
        use crate::voice::VoicePhase;
        let voice = crate::voice::voice_get_state(app.state());
        if !matches!(
            voice.phase,
            VoicePhase::Idle | VoicePhase::Error | VoicePhase::Success
        ) {
            return Err("请先完成或取消当前录音，再更换麦克风".into());
        }
        crate::audio::validate_microphone_source(&next.microphone_source)?;
    }
    if !["auto", "zh", "en", "ja"].contains(&next.stt_language.as_str()) {
        return Err("不支持的识别语言。".into());
    }
    if !["soft", "droplet", "tap"].contains(&next.start_sound.as_str()) {
        return Err("不支持的启动提示音。".into());
    }
    let configured = [
        &next.shortcut_record,
        &next.shortcut_cancel,
        &next.shortcut_finish,
    ]
    .into_iter()
    .map(|value| super::store::shortcut_to_tauri(value))
    .collect::<Result<Vec<_>, _>>()?;
    let mut ids = std::collections::HashSet::new();
    for value in &configured {
        if value == "Fn" {
            continue;
        }
        let shortcut = value.parse::<Shortcut>().map_err(|e| e.to_string())?;
        if !ids.insert(shortcut.id()) {
            return Err("开始、取消和完成的快捷键不能相同。".into());
        }
    }
    let old_record = super::store::shortcut_to_tauri(&previous.shortcut_record)?;
    let new_record = &configured[0];
    if new_record != "Fn"
        && (!new_record.contains('+')
            || ["Super+W", "Super+Q", "Super+C", "Super+V", "Super+X"]
                .contains(&new_record.as_str()))
    {
        return Err("录音快捷键请使用 Fn 或组合键，并避开关闭窗口、退出和剪贴板快捷键。".into());
    }
    let changed = old_record != *new_record;
    if changed && new_record != "Fn" {
        app.global_shortcut()
            .register(new_record.as_str())
            .map_err(|e| format!("无法注册快捷键 {}: {e}", next.shortcut_record))?;
    }
    *state.settings.write().map_err(|e| e.to_string())? = next;
    if let Err(error) = state.save_settings() {
        *state.settings.write().map_err(|e| e.to_string())? = previous;
        if changed && new_record != "Fn" {
            let _ = app.global_shortcut().unregister(new_record.as_str());
        }
        return Err(error);
    }
    if changed && old_record != "Fn" {
        if let Err(error) = app.global_shortcut().unregister(old_record.as_str()) {
            log::warn!("旧录音快捷键 {old_record} 注销失败: {error}");
        }
    }
    Ok(())
}
