use crate::settings::SettingsStore;
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::Manager;
mod download;

const ASR_URL: &str = "http://127.0.0.1:11436";
const RUNTIME_FILES: &[&str] = &[
    "Qwen3-ASR-1.7B-bf16/model.safetensors",
    "Qwen3-ASR-1.7B-bf16/config.json",
    "Qwen3-ASR-1.7B-bf16/tokenizer_config.json",
    "silero-vad/model.safetensors",
    "silero-vad/config.json",
    ".venv/bin/python",
    "python/bin/python3.13",
    "ollama-runtime/ollama",
    "ollama-runtime/llama-server",
    "editor-models/manifests/registry.ollama.ai/library/qwen3.5/4b",
];

fn runtime_status(phase: &str, message: &str, ready: bool) {
    crate::native::ui::update(serde_json::json!({
        "runtime": {"phase": phase, "message": message, "ready": ready}
    }));
}

#[derive(Default)]
pub struct LocalRuntime {
    child: Mutex<Option<Child>>,
    stopped: AtomicBool,
    preparing: AtomicBool,
    startup: tokio::sync::Mutex<()>,
}

struct PreparationGuard<'a>(&'a AtomicBool);
impl Drop for PreparationGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl LocalRuntime {
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.clear_child();
    }
    fn clear_child(&self) {
        if let Ok(mut current) = self.child.lock() {
            if let Some(mut child) = current.take() {
                stop_child(&mut child);
            }
        }
    }
}

fn stop_child(child: &mut Child) {
    // SIGTERM lets the sidecar stop its private model service before we reap it.
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    for _ in 0..20 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for LocalRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn client(timeout: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(timeout))
        .build()
        .map_err(|e| e.to_string())
}

fn model_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let bundled = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("quiettype-runtime");
    let cached = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("runtime-v1");
    if installed(&cached) {
        return fs::canonicalize(cached).map_err(|e| e.to_string());
    }
    if installed(&bundled) {
        return fs::canonicalize(bundled).map_err(|e| e.to_string());
    }
    let dir = app
        .state::<SettingsStore>()
        .settings
        .read()
        .map_err(|e| e.to_string())?
        .local_model_dir
        .clone();
    if dir.trim().is_empty() {
        return Err("语音识别文件未配置，请重新安装 QuietType。".into());
    }
    fs::canonicalize(dir).map_err(|_| "本地模型文件夹不存在，请检查位置。".into())
}

fn installed(root: &Path) -> bool {
    let speech = ["model.safetensors", "config.json", "tokenizer_config.json"]
        .iter()
        .all(|file| root.join("Qwen3-ASR-1.7B-bf16").join(file).is_file())
        && ["model.safetensors", "config.json"]
            .iter()
            .all(|file| root.join("silero-vad").join(file).is_file())
        && root.join(".venv/bin/python").is_file();
    let distributed = root.join("python").exists()
        || root.join("runtime-version.json").exists()
        || root.file_name().is_some_and(|name| name == "runtime-v1");
    speech
        && (!distributed
            || runtime_version_supported(root)
                && RUNTIME_FILES.iter().all(|file| root.join(file).is_file()))
}

fn runtime_version_supported(root: &Path) -> bool {
    fs::read(root.join("runtime-version.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|version| {
            version["version"] == 1
                && version["python"] == "3.13"
                && version["editor"] == "qwen3.5:4b"
        })
}

fn verify_runtime(root: &Path) -> Result<(), String> {
    if !installed(root) {
        return Err("本地语音文件不完整或版本不兼容，请重新下载完整安装包".into());
    }
    let manifest: std::collections::BTreeMap<String, String> = serde_json::from_slice(
        &fs::read(root.join("SHA256.json")).map_err(|e| format!("无法读取文件校验清单：{e}"))?,
    )
    .map_err(|e| format!("文件校验清单无效：{e}"))?;
    if manifest.is_empty() {
        return Err("文件校验清单为空，请重新下载完整安装包".into());
    }
    let canonical = root.canonicalize().map_err(|e| e.to_string())?;
    runtime_bytes(root, &canonical)?;
    for name in RUNTIME_FILES
        .iter()
        .copied()
        .chain(["runtime-version.json"])
    {
        if !root.join(name).is_symlink() && !manifest.contains_key(name) {
            return Err("文件校验清单缺少必需文件，请重新下载完整安装包".into());
        }
    }
    let mut checksums = String::new();
    for (name, digest) in manifest {
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || name.contains(['\n', '\r', '\\'])
            || !Path::new(&name)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || !root
                .join(&name)
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(&canonical)
        {
            return Err("文件校验清单包含无效路径或校验值".into());
        }
        checksums.push_str(&format!("{digest}  {name}\n"));
    }
    let mut child = Command::new("/usr/bin/shasum")
        .args(["-a", "256", "-c", "-"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("无法开始文件校验：{e}"))?;
    let written = child
        .stdin
        .take()
        .ok_or("无法读取校验进程输入")?
        .write_all(checksums.as_bytes());
    let status = child.wait().map_err(|e| e.to_string())?;
    written.map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("本地语音文件校验失败，请重新下载完整安装包".into());
    }
    Ok(())
}

fn runtime_bytes(directory: &Path, root: &Path) -> Result<u64, String> {
    let mut bytes = 0;
    for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            if !path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
            {
                return Err("安装资源包含指向外部的链接，请重新下载完整安装包".into());
            }
        } else if metadata.is_dir() {
            bytes += runtime_bytes(&path, root)?;
        } else if metadata.is_file() {
            bytes += metadata.len();
        } else {
            return Err("安装资源包含不支持的文件类型".into());
        }
    }
    Ok(bytes)
}

fn require_space(required: u64, available: u64) -> Result<(), String> {
    if available < required.saturating_add(64 * 1024 * 1024) {
        return Err(format!(
            "磁盘空间不足，准备本地语音还需要至少 {:.1} GB 可用空间，请释放空间后重试",
            required as f64 / 1_000_000_000.0 + 0.1
        ));
    }
    Ok(())
}

fn available_space(directory: &Path) -> Result<u64, String> {
    let output = Command::new("/bin/df")
        .args(["-Pk"])
        .arg(directory)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("无法检查磁盘可用空间，请稍后重试".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok())
        .map(|blocks| blocks * 1024)
        .ok_or_else(|| "无法读取磁盘可用空间".into())
}

fn remove_owned_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        }
        Ok(_) => Err("运行文件暂存路径异常，已保留原文件".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn cleanup_staging(data: &Path) -> Result<(), String> {
    for item in fs::read_dir(data).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let name = item.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix("runtime-v1-install-"))
            .filter(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        else {
            continue;
        };
        let alive = Command::new("/bin/kill")
            .args(["-0", pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| e.to_string())?
            .success();
        if !alive {
            remove_owned_directory(&item.path())?;
        }
    }
    Ok(())
}

fn runtime_paths(data: &Path) -> Result<(PathBuf, PathBuf), String> {
    fs::create_dir_all(data).map_err(|e| e.to_string())?;
    cleanup_staging(data)?;
    let target = data.join("runtime-v1");
    let previous = data.join("runtime-v1-previous");
    for path in [&target, &previous] {
        if fs::symlink_metadata(path)
            .is_ok_and(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
        {
            return Err("本地运行目录类型异常，已保留原文件，请重新安装完整版本".into());
        }
    }
    if !target.exists() && previous.exists() {
        fs::rename(&previous, &target).map_err(|e| e.to_string())?;
    }
    Ok((target, previous))
}

fn commit_runtime(temporary: &Path, target: &Path, previous: &Path) -> Result<(), String> {
    remove_owned_directory(previous)?;
    if target.exists() {
        fs::rename(target, previous).map_err(|e| e.to_string())?;
    }
    if let Err(error) = fs::rename(temporary, target) {
        if previous.exists() {
            let _ = fs::rename(previous, target);
        }
        return Err(format!("无法完成本地语音安装，原文件已保留：{error}"));
    }
    remove_owned_directory(previous)
}

fn install_runtime(source: &Path, data: &Path, check_cache: bool) -> Result<(), String> {
    let (target, previous) = runtime_paths(data)?;
    if check_cache && installed(&target) {
        runtime_status("verifying", "正在校验本地语音文件…", false);
    }
    if installed(&target) && (!check_cache || verify_runtime(&target).is_ok()) {
        remove_owned_directory(&previous)?;
        return Ok(());
    }
    if !source.join("runtime-version.json").is_file() {
        return if target.exists() {
            Err("本地语音文件需要修复，请重新下载完整安装包；设置和历史会保留".into())
        } else {
            Ok(())
        };
    }
    let bytes = runtime_bytes(source, &source.canonicalize().map_err(|e| e.to_string())?)?;
    require_space(bytes, available_space(data)?)?;
    let temporary = data.join(format!("runtime-v1-install-{}", std::process::id()));
    remove_owned_directory(&temporary)?;
    let result = (|| {
        runtime_status(
            "installing",
            "正在准备本地语音文件，首次安装需要一些时间…",
            false,
        );
        let status = Command::new("/usr/bin/ditto")
            .arg(source)
            .arg(&temporary)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("本地语音文件复制失败，请释放磁盘空间后重试".into());
        }
        runtime_status("verifying", "正在校验本地语音文件…", false);
        verify_runtime(&temporary)?;
        commit_runtime(&temporary, &target, &previous)
    })();
    let cleanup = remove_owned_directory(&temporary);
    result.and(cleanup)
}

fn preserve_runtime(app: &tauri::AppHandle, check_cache: bool) -> Result<(), String> {
    let source = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("quiettype-runtime");
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if source.join("runtime-version.json").is_file() {
        return install_runtime(&source, &data, check_cache);
    }
    let (cached, previous) = runtime_paths(&data)?;
    if installed(&cached) && (!check_cache || verify_runtime(&cached).is_ok()) {
        return remove_owned_directory(&previous);
    }
    if !cached.exists() {
        let configured = app
            .state::<SettingsStore>()
            .settings
            .read()
            .map_err(|e| e.to_string())?
            .local_model_dir
            .clone();
        if installed(Path::new(&configured)) {
            return Ok(());
        }
    }
    download::prepare(
        &app.path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("runtime-download.json"),
        &data,
        &app.state::<LocalRuntime>(),
    )
}

async fn running(root: &std::path::Path) -> bool {
    let Ok(client) = client(2) else {
        return false;
    };
    let Ok(response) = client.get(format!("{ASR_URL}/health")).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(value) = response.json::<serde_json::Value>().await else {
        return false;
    };
    value["protocol"] == 3
        && value["model"] == "Qwen3-ASR-1.7B-bf16"
        && value["modelsDir"]
            .as_str()
            .is_some_and(|dir| std::path::Path::new(dir) == root)
}

pub(crate) async fn ensure(app: &tauri::AppHandle) -> Result<(), String> {
    let result = ensure_inner(app, false).await;
    if let Err(error) = &result {
        runtime_status("error", error, false);
    }
    result
}

async fn ensure_inner(app: &tauri::AppHandle, check_cache: bool) -> Result<(), String> {
    let runtime = app.state::<LocalRuntime>();
    let _startup = runtime.startup.lock().await;
    if runtime.stopped.load(Ordering::SeqCst) {
        return Err("QuietType 正在退出。".into());
    }
    let prepare_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || preserve_runtime(&prepare_handle, check_cache))
        .await
        .map_err(|e| e.to_string())??;
    let root = model_dir(app)?;
    if !installed(&root) {
        return Err("语音识别文件缺失，请重新安装 QuietType。".into());
    }
    if running(&root).await {
        return Ok(());
    }
    let app_handle = app.clone();
    let spawn_root = root.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let root = spawn_root;
        let state = app_handle.state::<LocalRuntime>();
        let mut current = state.child.lock().map_err(|e| e.to_string())?;
        if state.stopped.load(Ordering::SeqCst) {
            return Err("QuietType 正在退出。".into());
        }
        if let Some(mut previous) = current.take() {
            stop_child(&mut previous);
        }
        let script = app_handle
            .path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("local_speech/server.py");
        let script = if script.is_file() {
            script
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sidecar/local_speech/server.py")
        };
        let log_dir = app_handle.path().app_log_dir().map_err(|e| e.to_string())?;
        fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("asr-runtime.log"))
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(root.join(".venv/bin/python"));
        command.env(
            "QUIETTYPE_VOCABULARY_PATH",
            app_handle
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())?
                .join("user-words.json"),
        );
        if root.join("python").is_dir() {
            command
                .env("PYTHONHOME", root.join("python"))
                .env(
                    "PYTHONPATH",
                    root.join(".venv/lib/python3.13/site-packages"),
                )
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .env("QUIETTYPE_OLLAMA_HOST", "127.0.0.1:11437")
                .env(
                    "QUIETTYPE_OLLAMA_BINARY",
                    root.join("ollama-runtime/ollama"),
                )
                .env("OLLAMA_MODELS", root.join("editor-models"))
                .env("OLLAMA_NO_CLOUD", "1");
        }
        let child = command
            .arg(script)
            .arg("--models-dir")
            .arg(&root)
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|e| format!("本地识别服务启动失败: {e}"))?;
        *current = Some(child);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    for _ in 0..40 {
        if running(&root).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    runtime.clear_child();
    Err("本地识别服务启动超时，请重新打开 QuietType 后重试。".into())
}

pub fn validate_asr(app: &tauri::AppHandle) -> Result<(), String> {
    if model_dir(app).is_ok_and(|root| installed(&root)) {
        Ok(())
    } else if app.state::<LocalRuntime>().preparing.load(Ordering::SeqCst) {
        Err("正在准备本地语音，完成后即可开始".into())
    } else {
        Err("语音识别文件缺失，请重新安装 QuietType。".into())
    }
}

pub async fn warm(app: tauri::AppHandle) -> Result<(), String> {
    prepare(app, false).await
}

pub async fn retry_prepare(app: tauri::AppHandle) -> Result<(), String> {
    use crate::voice::VoicePhase;
    let voice = crate::voice::voice_get_state(app.state());
    if !matches!(
        voice.phase,
        VoicePhase::Idle | VoicePhase::Error | VoicePhase::Success
    ) {
        return Err("请先完成或取消当前录音，再重试准备本地语音".into());
    }
    prepare(app, true).await
}

async fn prepare(app: tauri::AppHandle, check_cache: bool) -> Result<(), String> {
    let runtime = app.state::<LocalRuntime>();
    if runtime.preparing.swap(true, Ordering::SeqCst) {
        return Err("本地语音正在准备，请稍候".into());
    }
    let _preparing = PreparationGuard(&runtime.preparing);
    runtime_status("checking", "正在检查本地语音…", false);
    let result = async {
        ensure_inner(&app, check_cache).await?;
        runtime_status(
            "loading",
            "正在加载本地语音，首次准备可能需要几分钟…",
            false,
        );
        let response = client(180)?
            .post(format!("{ASR_URL}/warm"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(response.text().await.unwrap_or_default());
        }
        Ok(())
    }
    .await;
    match &result {
        Ok(()) => runtime_status("ready", "本地语音已就绪", true),
        Err(error) => runtime_status("error", error, false),
    }
    result
}

fn job_id(session_id: u64) -> String {
    format!("{}-{session_id}", std::process::id())
}

pub async fn transcribe(
    app: &tauri::AppHandle,
    pcm: Vec<u8>,
    language: Option<&str>,
    session_id: u64,
    english: bool,
) -> Result<Option<String>, String> {
    ensure(app).await?;
    let response = client(180)?
        .post(format!("{ASR_URL}/transcribe"))
        .header("X-Session-Id", job_id(session_id))
        .header("X-Language", language.unwrap_or("auto"))
        .header("X-Output-Mode", if english { "english" } else { "clean" })
        .header("Content-Type", "application/octet-stream")
        .body(pcm)
        .send()
        .await
        .map_err(|e| format!("本地识别失败: {e}"))?;
    text_response(response).await
}

pub async fn cancel_transcription(session_id: u64) {
    if let Ok(client) = client(2) {
        let _ = client
            .post(format!("{ASR_URL}/cancel"))
            .header("X-Session-Id", job_id(session_id))
            .send()
            .await;
    }
}

async fn text_response(response: reqwest::Response) -> Result<Option<String>, String> {
    let status = response.status();
    let json: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(json["error"].as_str().unwrap_or("识别请求失败").into());
    }
    clean_text(json["text"].as_str(), json["speech"] == false)
}

fn clean_text(text: Option<&str>, no_speech: bool) -> Result<Option<String>, String> {
    if no_speech {
        return Ok(None);
    }
    text.filter(|text| !text.trim().is_empty())
        .map(|text| Some(text.to_owned()))
        .ok_or("模型未返回有效文字".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_empty_and_whitespace_outputs() {
        assert!(clean_text(None, false).is_err());
        assert!(clean_text(Some(" \n"), false).is_err());
        assert_eq!(
            clean_text(Some(" 完成。 \n"), false).unwrap().as_deref(),
            Some(" 完成。 \n")
        );
        assert!(clean_text(Some(""), true).unwrap().is_none());
        assert!(!installed(std::path::Path::new("/no-such-model-dir")));
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("quiettype-runtime-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn runtime(&self) -> PathBuf {
            let root = self.0.join("source");
            fs::create_dir(&root).unwrap();
            for name in RUNTIME_FILES {
                let file = root.join(name);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, b"tiny runtime fixture").unwrap();
            }
            fs::write(
                root.join("runtime-version.json"),
                r#"{"version":1,"python":"3.13","editor":"qwen3.5:4b"}"#,
            )
            .unwrap();
            let mut manifest = std::collections::BTreeMap::new();
            for name in RUNTIME_FILES
                .iter()
                .copied()
                .chain(["runtime-version.json"])
            {
                let output = Command::new("/usr/bin/shasum")
                    .args(["-a", "256"])
                    .arg(root.join(name))
                    .output()
                    .unwrap();
                assert!(output.status.success());
                let digest = String::from_utf8(output.stdout)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_owned();
                manifest.insert(name, digest);
            }
            fs::write(
                root.join("SHA256.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            root
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn runtime_install_repairs_incomplete_cache_without_touching_user_files() {
        let fixture = Fixture::new();
        let source = fixture.runtime();
        let data = fixture.0.join("data");
        fs::create_dir_all(data.join("runtime-v1")).unwrap();
        fs::write(data.join("runtime-v1/broken"), b"old cache").unwrap();
        fs::write(data.join("history.db"), b"history must survive").unwrap();
        fs::create_dir(data.join("runtime-v1-install-999999999")).unwrap();
        fs::create_dir(data.join("runtime-v1-install-personal")).unwrap();
        install_runtime(&source, &data, false).unwrap();
        verify_runtime(&data.join("runtime-v1")).unwrap();
        assert_eq!(
            fs::read(data.join("history.db")).unwrap(),
            b"history must survive"
        );
        assert!(!data.join("runtime-v1/broken").exists());
        assert!(!data.join("runtime-v1-previous").exists());
        assert!(!data.join("runtime-v1-install-999999999").exists());
        assert!(data.join("runtime-v1-install-personal").exists());
        assert!(!data
            .join(format!("runtime-v1-install-{}", std::process::id()))
            .exists());
    }

    #[test]
    fn failed_runtime_verification_keeps_cache_and_cleans_staging() {
        let fixture = Fixture::new();
        let source = fixture.runtime();
        fs::write(source.join(RUNTIME_FILES[0]), b"damaged download").unwrap();
        let data = fixture.0.join("data");
        fs::create_dir_all(data.join("runtime-v1")).unwrap();
        fs::write(data.join("runtime-v1/keep"), b"existing files").unwrap();
        assert!(install_runtime(&source, &data, false).is_err());
        assert_eq!(
            fs::read(data.join("runtime-v1/keep")).unwrap(),
            b"existing files"
        );
        assert!(!data
            .join(format!("runtime-v1-install-{}", std::process::id()))
            .exists());
        assert!(!data.join("runtime-v1-previous").exists());
    }

    #[test]
    fn runtime_version_space_and_external_links_are_checked() {
        let fixture = Fixture::new();
        let source = fixture.runtime();
        assert!(installed(&source));
        assert!(require_space(100, 101).is_err());
        assert!(require_space(100, 100 + 64 * 1024 * 1024).is_ok());
        fs::write(source.join("runtime-version.json"), r#"{"version":2}"#).unwrap();
        assert!(!installed(&source));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&fixture.0, source.join("external")).unwrap();
            assert!(runtime_bytes(&source, &source.canonicalize().unwrap()).is_err());
        }
    }

    #[test]
    fn ordinary_start_reuses_cache_and_manual_retry_repairs_corruption() {
        let fixture = Fixture::new();
        let source = fixture.runtime();
        let data = fixture.0.join("data");
        install_runtime(&source, &data, false).unwrap();
        let cached_model = data.join("runtime-v1").join(RUNTIME_FILES[0]);
        fs::write(&cached_model, b"damaged later").unwrap();
        install_runtime(&source, &data, false).unwrap();
        assert_eq!(fs::read(&cached_model).unwrap(), b"damaged later");
        install_runtime(&source, &data, true).unwrap();
        verify_runtime(&data.join("runtime-v1")).unwrap();
        fs::rename(data.join("runtime-v1"), data.join("runtime-v1-previous")).unwrap();
        install_runtime(&source, &data, false).unwrap();
        assert!(installed(&data.join("runtime-v1")));
        assert!(!data.join("runtime-v1-previous").exists());
    }

    #[cfg(unix)]
    #[test]
    fn repair_does_not_follow_an_external_cache_link() {
        let fixture = Fixture::new();
        let source = fixture.runtime();
        let data = fixture.0.join("data");
        fs::create_dir(&data).unwrap();
        std::os::unix::fs::symlink(&source, data.join("runtime-v1")).unwrap();
        assert!(install_runtime(&source, &data, false).is_err());
        assert!(data.join("runtime-v1").is_symlink());
        verify_runtime(&source).unwrap();
    }

    #[test]
    fn literal_snippet_keeps_outer_newlines() {
        assert_eq!(
            clean_text(Some("\n  署名\n"), false).unwrap().as_deref(),
            Some("\n  署名\n")
        );
    }
}
