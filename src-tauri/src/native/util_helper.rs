use libloading::{Library, Symbol};
use serde::Deserialize;
use std::ffi::CStr;
use std::sync::OnceLock;
use tauri::AppHandle;

static LIB: OnceLock<Library> = OnceLock::new();

pub fn init(app: &AppHandle) -> Result<(), String> {
    let path = super::dylib_path(app, "libUtilHelper.dylib");
    init_from_path(&path)
}

pub(crate) fn init_from_path(path: &std::path::Path) -> Result<(), String> {
    log::info!("Loading UtilHelper dylib from {:?}", path);
    let lib = unsafe { Library::new(&path) }
        .map_err(|e| format!("Failed to load libUtilHelper.dylib from {:?}: {}", path, e))?;
    LIB.set(lib)
        .map_err(|_| "libUtilHelper already initialized".to_string())?;
    Ok(())
}

pub fn download_proxy() -> Result<Option<String>, String> {
    unsafe {
        let function: Symbol<unsafe extern "C" fn(*mut std::os::raw::c_char, u32) -> u32> = lib()?
            .get(b"quiet_download_proxy")
            .map_err(|e| e.to_string())?;
        let mut bytes = [0u8; 4096];
        if function(bytes.as_mut_ptr().cast(), bytes.len() as u32) > bytes.len() as u32 {
            return Err("系统代理地址过长，请检查网络设置".into());
        }
        let value = CStr::from_bytes_until_nul(&bytes)
            .map_err(|e| e.to_string())?
            .to_str()
            .map_err(|e| e.to_string())?;
        Ok((!value.is_empty()).then(|| value.to_owned()))
    }
}

fn lib() -> Result<&'static Library, String> {
    LIB.get()
        .ok_or_else(|| "libUtilHelper not initialized".to_string())
}

pub fn prepare_installation() -> Result<bool, String> {
    unsafe {
        let function: Symbol<unsafe extern "C" fn() -> bool> = lib()?
            .get(b"quiet_prepare_installation")
            .map_err(|e| e.to_string())?;
        Ok(function())
    }
}

pub type AudioSamplesCallback = unsafe extern "C" fn(*const f32, u32, *mut std::ffi::c_void);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneDevice {
    pub name: String,
    pub built_in: bool,
    pub headset: bool,
    pub default_input: bool,
    pub default_output: bool,
}

pub fn microphone_devices() -> Result<Vec<MicrophoneDevice>, String> {
    unsafe {
        let function: Symbol<unsafe extern "C" fn(*mut std::os::raw::c_char, u32) -> u32> = lib()?
            .get(b"microphone_devices_json")
            .map_err(|e| e.to_string())?;
        let size = function(std::ptr::null_mut(), 0);
        if size == 0 || size > 1_048_576 {
            return Err("无法读取麦克风列表".into());
        }
        let mut bytes = vec![0u8; size as usize];
        if function(bytes.as_mut_ptr().cast(), size) > size {
            return Err("麦克风列表已变化，请重试".into());
        }
        let json = CStr::from_bytes_until_nul(&bytes).map_err(|e| e.to_string())?;
        serde_json::from_slice(json.to_bytes()).map_err(|e| format!("麦克风列表无效：{e}"))
    }
}

pub unsafe fn start_voice_capture(
    callback: AudioSamplesCallback,
    context: *mut std::ffi::c_void,
    sample_rate: &mut f64,
) -> Result<*mut std::ffi::c_void, String> {
    let func: Symbol<
        unsafe extern "C" fn(
            AudioSamplesCallback,
            *mut std::ffi::c_void,
            *mut f64,
        ) -> *mut std::ffi::c_void,
    > = lib()?
        .get(b"start_voice_capture")
        .map_err(|e| e.to_string())?;
    let handle = func(callback, context, sample_rate);
    if handle.is_null() {
        Err("系统回声消除未能启动".into())
    } else {
        Ok(handle)
    }
}

pub unsafe fn stop_voice_capture(handle: *mut std::ffi::c_void) {
    if let Ok(library) = lib() {
        if let Ok(func) =
            library.get::<unsafe extern "C" fn(*mut std::ffi::c_void)>(b"stop_voice_capture")
        {
            func(handle);
        }
    }
}

pub fn communication_audio_active() -> Result<bool, String> {
    unsafe {
        let func: Symbol<unsafe extern "C" fn() -> bool> = lib()?
            .get(b"communication_audio_active")
            .map_err(|e| e.to_string())?;
        Ok(func())
    }
}

pub fn prepare_voice_capture() -> Result<(), String> {
    unsafe {
        let func: Symbol<unsafe extern "C" fn()> = lib()?
            .get(b"prepare_voice_capture")
            .map_err(|e| e.to_string())?;
        func();
    }
    Ok(())
}

pub fn microphone_authorization_status() -> Result<i32, String> {
    let lib = lib()?;
    unsafe {
        let func: Symbol<unsafe extern "C" fn() -> i32> = lib
            .get(b"microphone_authorization_status")
            .map_err(|e| e.to_string())?;
        Ok(func())
    }
}

pub fn request_microphone_permission() -> Result<bool, String> {
    let lib = lib()?;
    unsafe {
        let func: Symbol<unsafe extern "C" fn() -> bool> = lib
            .get(b"request_microphone_permission")
            .map_err(|e| e.to_string())?;
        Ok(func())
    }
}

pub fn request_accessibility_permission() -> Result<bool, String> {
    let lib = lib()?;
    unsafe {
        let func: Symbol<unsafe extern "C" fn() -> bool> = lib
            .get(b"request_accessibility_permission")
            .map_err(|e| e.to_string())?;
        Ok(func())
    }
}

pub fn check_accessibility_permission() -> Result<bool, String> {
    unsafe {
        let func: Symbol<unsafe extern "C" fn() -> bool> = lib()?
            .get(b"check_accessibility_permission")
            .map_err(|e| e.to_string())?;
        Ok(func())
    }
}
