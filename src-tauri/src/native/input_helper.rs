use libloading::{Library, Symbol};
use std::ffi::CString;
use std::sync::OnceLock;
use tauri::AppHandle;

static LIB: OnceLock<Library> = OnceLock::new();

pub fn init(app: &AppHandle) -> Result<(), String> {
    let path = super::dylib_path(app, "libInputHelper.dylib");
    let library = unsafe { Library::new(&path) }
        .map_err(|e| format!("Failed to load libInputHelper.dylib from {:?}: {}", path, e))?;
    LIB.set(library)
        .map_err(|_| "libInputHelper already initialized".to_string())?;
    unsafe {
        let init: Symbol<unsafe extern "C" fn()> = lib()?
            .get(b"init_input_tracking")
            .map_err(|e| e.to_string())?;
        init();
    }
    Ok(())
}

fn lib() -> Result<&'static Library, String> {
    LIB.get()
        .ok_or_else(|| "libInputHelper not initialized".to_string())
}

pub fn refusal_message(code: i32) -> &'static str {
    match code {
        -1 => "辅助功能未授权，请在系统设置中允许 QuietType；文字可复制",
        -3 => "请点击其他软件的输入框；文字可复制",
        -4 | -11 => "输入框暂未就绪，请点击输入框后重试；文字可复制",
        -5 => "焦点与当前应用不一致，已保留文字",
        -6 => "当前焦点不是可编辑输入框；文字可复制",
        -7 => "安全输入框不自动填入；文字可复制",
        -8 => "填入时焦点已变化，已保留文字",
        -9 | -10 => "粘贴未能执行；文字可复制",
        _ => "文字已识别，请复制使用",
    }
}

fn session_call(name: &[u8], session_id: u64) -> Result<bool, String> {
    unsafe {
        let func: Symbol<unsafe extern "C" fn(u64) -> bool> =
            lib()?.get(name).map_err(|e| e.to_string())?;
        Ok(func(session_id))
    }
}

pub fn begin_input_session(session_id: u64) -> Result<bool, String> {
    session_call(b"begin_input_session", session_id)
}
pub fn clear_input_target() {
    unsafe {
        if let Ok(lib) = lib() {
            if let Ok(func) = lib.get::<unsafe extern "C" fn()>(b"clear_input_target") {
                func();
            }
        }
    }
}

pub fn insert_text(text: &str, session_id: u64) -> Result<i32, String> {
    let c_text = CString::new(text).map_err(|e| e.to_string())?;
    unsafe {
        let func: Symbol<unsafe extern "C" fn(*const std::os::raw::c_char, u64) -> i32> =
            lib()?.get(b"insert_text").map_err(|e| e.to_string())?;
        Ok(func(c_text.as_ptr(), session_id))
    }
}
