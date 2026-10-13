mod ai;
mod audio;
mod database;
mod native;
mod personal;
mod settings;
mod voice;

use database::Database;
use settings::SettingsStore;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;

#[derive(Default)]
struct ShortcutGesture {
    last_event_at: Option<Instant>,
}

impl ShortcutGesture {
    fn press(&mut self, now: Instant) -> bool {
        // Key repeat must not toggle. Recover if macOS swallowed the release
        // while showing a permission sheet, rather than locking the key forever.
        self.last_event_at.replace(now).map_or(true, |last| {
            now.duration_since(last) >= Duration::from_secs(2)
        })
    }
    fn release(&mut self) {
        self.last_event_at = None;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            let db = Database::new()?;
            db.run_migrations()?;
            db.purge_expired()?;
            app.manage(db);
            app.manage(SettingsStore::new()?);
            app.manage(audio::AudioRecorder::new());
            app.manage(voice::VoiceSession::new());
            app.manage(ai::local::LocalRuntime::default());
            native::init(app.handle()).map_err(std::io::Error::other)?;
            if native::util_helper::prepare_installation().map_err(std::io::Error::other)? {
                return Ok(());
            }

            let handle = app.handle().clone();
            let gesture = Arc::new(Mutex::new(ShortcutGesture::default()));
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |_app, shortcut, event| {
                        use tauri_plugin_global_shortcut::ShortcutState;
                        if event.state() == ShortcutState::Pressed
                            && voice::is_cancel_shortcut(&handle, shortcut)
                        {
                            let handle = handle.clone();
                            tauri::async_runtime::spawn(async move {
                                voice::cancel_from_shortcut(handle);
                            });
                            return;
                        }
                        if event.state() == ShortcutState::Pressed
                            && voice::is_finish_shortcut(&handle, shortcut)
                        {
                            let handle = handle.clone();
                            tauri::async_runtime::spawn(async move {
                                voice::finish_from_shortcut(handle);
                            });
                            return;
                        }
                        if native::ui::capturing_shortcut()
                            || !voice::is_record_shortcut(&handle, shortcut)
                        {
                            return;
                        }
                        match event.state() {
                            ShortcutState::Pressed => {
                                let accepted = gesture
                                    .lock()
                                    .map(|mut value| value.press(Instant::now()))
                                    .unwrap_or(false);
                                if !accepted {
                                    return;
                                }
                                let handle = handle.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Err(error) = voice::toggle(handle).await {
                                        log::error!("voice shortcut failed: {error}");
                                    }
                                });
                            }
                            ShortcutState::Released => {
                                if let Ok(mut value) = gesture.lock() {
                                    value.release();
                                }
                            }
                        }
                    })
                    .build(),
            )?;
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            let record = app
                .state::<SettingsStore>()
                .settings
                .read()
                .map_err(|e| e.to_string())?
                .shortcut_record
                .clone();
            let record = settings::store::shortcut_to_tauri(&record)?;
            if record != "Fn" {
                if let Err(error) = app.global_shortcut().register(record.as_str()) {
                    log::warn!("语音快捷键 {record} 不可用，应用继续启动: {error}");
                }
            }
            native::ui::init(app.handle()).map_err(std::io::Error::other)?;
            let history_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                loop {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    match history_app.state::<Database>().purge_expired() {
                        Ok(0) => {}
                        Ok(_) => {
                            let _ = history_app.emit("history-changed", ());
                        }
                        Err(error) => log::warn!("清理过期记录失败: {error}"),
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building QuietType")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<ai::local::LocalRuntime>().stop();
                app.state::<audio::AudioRecorder>().cancel();
            }
            if let tauri::RunEvent::Reopen { .. } = event {
                native::ui::show();
            }
        });
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;
    #[test]
    fn ignores_key_repeat_but_allows_the_next_tap() {
        let started = Instant::now();
        let mut gesture = ShortcutGesture::default();
        assert!(gesture.press(started));
        assert!(!gesture.press(started + Duration::from_millis(500)));
        assert!(!gesture.press(started + Duration::from_millis(800)));
        gesture.release();
        assert!(gesture.press(started + Duration::from_millis(900)));
        assert!(gesture.press(started + Duration::from_secs(4)));
    }
}
