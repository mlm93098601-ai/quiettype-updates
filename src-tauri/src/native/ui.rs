use libloading::{Library, Symbol};
use std::ffi::{CStr, CString};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    OnceLock,
};
use tauri::{AppHandle, Listener, Manager};

static LIB: OnceLock<Library> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();
static HISTORY_REVISION: AtomicU64 = AtomicU64::new(0);
static CAPTURING_SHORTCUT: AtomicBool = AtomicBool::new(false);
static FN_ACTIONS: OnceLock<tokio::sync::mpsc::UnboundedSender<FnAction>> = OnceLock::new();

enum FnAction {
    Press(bool),
    English,
    Cancel,
    Release,
}

pub fn capturing_shortcut() -> bool {
    CAPTURING_SHORTCUT.load(Ordering::SeqCst)
}

pub fn update(value: serde_json::Value) {
    if let (Some(lib), Ok(json)) = (LIB.get(), CString::new(value.to_string())) {
        unsafe {
            if let Ok(f) =
                lib.get::<unsafe extern "C" fn(*const std::os::raw::c_char)>(b"quiet_ui_update")
            {
                f(json.as_ptr());
            }
        }
    }
}
pub fn show() {
    call(b"quiet_ui_show");
}
pub fn hide_hud() {
    call(b"quiet_ui_hide_hud");
}
fn call(name: &[u8]) {
    if let Some(lib) = LIB.get() {
        unsafe {
            if let Ok(f) = lib.get::<unsafe extern "C" fn()>(name) {
                f();
            }
        }
    }
}
fn refresh(app: &AppHandle) -> Result<(), String> {
    let settings = crate::settings::commands::get_settings(app.state())?;
    let shortcuts =
        crate::settings::commands::get_shortcut_registration_status(app.clone(), app.state())?;
    let voice = crate::voice::voice_get_state(app.state());
    let accessibility = crate::native::util_helper::check_accessibility_permission()?;
    let microphone = crate::native::util_helper::microphone_authorization_status()?;
    update(
        serde_json::json!({"settings": settings, "shortcutReady": shortcuts, "voice": voice,
        "accessibilityGranted": accessibility, "microphoneStatus": microphone}),
    );
    Ok(())
}

pub fn record_first_input(app: &AppHandle) {
    if app
        .state::<crate::settings::SettingsStore>()
        .settings
        .read()
        .map(|settings| !settings.setup_complete)
        .unwrap_or(false)
    {
        // A dispatched paste is not proof; ask the user to confirm the visible text.
        update(serde_json::json!({"firstInputSent": true}));
    }
}
unsafe extern "C" fn action(pointer: *const std::os::raw::c_char) {
    if pointer.is_null() {
        return;
    }
    let Ok(packet) =
        serde_json::from_str::<serde_json::Value>(&CStr::from_ptr(pointer).to_string_lossy())
    else {
        return;
    };
    let Some(app) = APP.get().cloned() else {
        return;
    };
    let name = packet["action"].as_str().unwrap_or("").to_string();
    let args = packet["args"].clone();
    let fn_action = match name.as_str() {
        "fnPress" => Some(FnAction::Press(args["english"].as_bool().unwrap_or(false))),
        "fnEnglish" => Some(FnAction::English),
        "fnCancel" => Some(FnAction::Cancel),
        "fnRelease" => Some(FnAction::Release),
        _ => None,
    };
    if let Some(event) = fn_action {
        if let Some(sender) = FN_ACTIONS.get() {
            let _ = sender.send(event);
        }
        return;
    }
    if name == "shortcutCapture" {
        CAPTURING_SHORTCUT.store(args["active"].as_bool().unwrap_or(false), Ordering::SeqCst);
        return;
    }
    let revision = if name == "history" {
        Some(HISTORY_REVISION.fetch_add(1, Ordering::SeqCst) + 1)
    } else {
        None
    };
    if name == "history" {
        update(serde_json::json!({"historyBusy": true}));
    }
    tauri::async_runtime::spawn(async move {
        let result: Result<(), String> = match name.as_str() {
            "toggle" => crate::voice::voice_toggle(app.clone()).await.map(|_| ()),
            "toggleEnglish" => crate::voice::toggle_mode(app.clone(), true)
                .await
                .map(|_| ()),
            "finish" => crate::voice::voice_finish(app.clone()).map(|_| ()),
            "cancel" => crate::voice::voice_cancel(app.clone()).map(|_| ()),
            "retry" => crate::voice::voice_retry(app.clone()).map(|_| ()),
            "reinsert" => crate::voice::retry_insertion(
                app.clone(),
                args["sessionId"].as_u64().unwrap_or(u64::MAX),
                args["delayMs"].as_u64().unwrap_or(0).min(2000),
            )
            .await
            .map(|_| ()),
            "refresh" => refresh(&app),
            "requestMicrophone" => {
                let granted = tauri::async_runtime::spawn_blocking(
                    crate::native::util_helper::request_microphone_permission,
                )
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| value);
                granted.and_then(|_| refresh(&app))
            }
            "prepare" => crate::ai::local::retry_prepare(app.clone()).await,
            "personal" => crate::personal::get(&app)
                .await
                .map(|value| update(serde_json::json!({"personal": value, "personalBusy": false}))),
            "personalChange" => {
                let change = args["action"].as_str().unwrap_or("").to_string();
                crate::personal::apply(&app, args).await.map(|value| update(serde_json::json!({"personal": value, "personalSaved": change, "personalBusy": false})))
            }
            "settings" => {
                crate::settings::commands::update_settings(app.clone(), app.state(), args)
                    .and_then(|_| refresh(&app))
            }
            "history" => {
                let query = args["query"].as_str().unwrap_or("").to_string();
                let offset = args["offset"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
                let rows = crate::database::commands::get_records(app.state(), &query, offset);
                if revision == Some(HISTORY_REVISION.load(Ordering::SeqCst)) {
                    rows.map(|rows| update(serde_json::json!({"history": rows, "append": offset > 0, "historyBusy": false})))
                } else {
                    Ok(())
                }
            }
            "historyDelete" => {
                HISTORY_REVISION.fetch_add(1, Ordering::SeqCst);
                crate::database::commands::delete_record(
                    app.state(),
                    args["id"].as_str().unwrap_or(""),
                )
                .map(|_| {
                    update(serde_json::json!({"historyChanged": true, "notice": "已删除记录"}))
                })
            }
            "historyClear" if args["confirmed"] == true => {
                HISTORY_REVISION.fetch_add(1, Ordering::SeqCst);
                crate::database::commands::clear_records(app.state()).map(|_| {
                    update(serde_json::json!({"historyChanged": true, "notice": "已清空记录"}))
                })
            }
            _ => Err("不支持的操作".into()),
        };
        if let Err(error) = result {
            if revision.is_some() && revision != Some(HISTORY_REVISION.load(Ordering::SeqCst)) {
                return;
            }
            update(
                serde_json::json!({"notice": error, "historyBusy": false, "personalBusy": false}),
            );
        }
    });
}
pub fn init(app: &AppHandle) -> Result<(), String> {
    let path = super::dylib_path(app, "libNativeUI.dylib");
    let lib = unsafe { Library::new(&path) }.map_err(|e| e.to_string())?;
    APP.set(app.clone())
        .map_err(|_| "native UI initialized twice")?;
    LIB.set(lib).map_err(|_| "native UI initialized twice")?;
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    FN_ACTIONS
        .set(sender)
        .map_err(|_| "Fn event worker initialized twice")?;
    let fn_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut session = None;
        while let Some(event) = events.recv().await {
            let result = match event {
                FnAction::Press(english) => {
                    match crate::voice::fn_press(fn_app.clone(), english).await {
                        Ok(started) => {
                            session = started;
                            Ok(())
                        }
                        Err(error) => {
                            session = None;
                            Err(error)
                        }
                    }
                }
                FnAction::English => {
                    if let Some(session_id) = session {
                        crate::voice::fn_english(&fn_app, session_id);
                    }
                    Ok(())
                }
                FnAction::Cancel => match session.take() {
                    Some(session_id) => crate::voice::fn_cancel(fn_app.clone(), session_id),
                    None => Ok(()),
                },
                FnAction::Release => {
                    session = None;
                    Ok(())
                }
            };
            if let Err(error) = result {
                update(serde_json::json!({"notice": error}));
            }
        }
    });
    unsafe {
        let f: Symbol<unsafe extern "C" fn(unsafe extern "C" fn(*const std::os::raw::c_char))> =
            LIB.get()
                .unwrap()
                .get(b"quiet_ui_init")
                .map_err(|e| e.to_string())?;
        f(action);
    }
    refresh(app)?;
    // Automatic insertion is always enabled. macOS owns the persistent grant;
    // the application requests it when missing and never revokes it on exit.
    if !crate::native::util_helper::check_accessibility_permission()? {
        let _ = crate::native::util_helper::request_accessibility_permission()?;
    }
    let rows = crate::database::commands::get_records(app.state(), "", 0)?;
    update(serde_json::json!({"history": rows}));
    app.listen("history-changed", move |_| {
        update(serde_json::json!({"historyChanged": true}));
    });
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(|| {
        if let Err(error) = super::util_helper::prepare_voice_capture() {
            log::warn!("audio preparation deferred: {error}");
        }
    });
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::ai::local::warm(handle).await {
            update(serde_json::json!({"notice": error}));
        }
    });
    Ok(())
}
