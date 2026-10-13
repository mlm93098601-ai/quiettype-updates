use crate::{audio::AudioRecorder, settings::SettingsStore};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

const MICROPHONE_START_TIMEOUT: Duration = Duration::from_secs(5);
const AUDIO_MONITOR_INTERVAL: Duration = Duration::from_millis(50);
const MAX_RECORDING_DURATION: Duration = Duration::from_secs(300);
const RECORDING_WARNING_SECONDS: u64 = 270;
const MAX_PCM_BYTES: usize = 300 * 32_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VoicePhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Transcribing,
    Retrying,
    Inserting,
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSnapshot {
    pub session_id: u64,
    pub phase: VoicePhase,
    pub status: String,
    pub error: Option<String>,
    pub transcript: Option<String>,
    pub can_retry: bool,
    pub can_insert: bool,
    pub injected: bool,
    pub elapsed_seconds: u64,
    pub limit_warning: bool,
}

struct VoiceInner {
    snapshot: VoiceSnapshot,
    retained_pcm: Option<Arc<Vec<u8>>>,
    english: bool,
}

pub struct VoiceSession {
    inner: Mutex<VoiceInner>,
    setup: Mutex<()>,
    capture_start: Mutex<()>,
}

impl VoiceSession {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VoiceInner {
                snapshot: VoiceSnapshot {
                    session_id: 0,
                    phase: VoicePhase::Idle,
                    status: String::new(),
                    error: None,
                    transcript: None,
                    can_retry: false,
                    can_insert: false,
                    injected: false,
                    elapsed_seconds: 0,
                    limit_warning: false,
                },
                retained_pcm: None,
                english: false,
            }),
            setup: Mutex::new(()),
            capture_start: Mutex::new(()),
        }
    }

    fn with_setup<T>(&self, action: impl FnOnce() -> T) -> T {
        let _setup = self.setup.lock().expect("voice setup poisoned");
        action()
    }

    fn is_starting(&self, session_id: u64) -> bool {
        let inner = self.inner.lock().expect("voice state poisoned");
        inner.snapshot.session_id == session_id && inner.snapshot.phase == VoicePhase::Starting
    }

    fn fn_active(&self, session_id: u64) -> bool {
        let inner = self.inner.lock().expect("voice state poisoned");
        inner.snapshot.session_id == session_id
            && matches!(
                inner.snapshot.phase,
                VoicePhase::Starting | VoicePhase::Recording
            )
    }

    fn set_fn_english(&self, session_id: u64) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.snapshot.session_id != session_id
            || !matches!(
                inner.snapshot.phase,
                VoicePhase::Starting | VoicePhase::Recording
            )
        {
            return false;
        }
        inner.english = true;
        true
    }

    fn start_current_capture(
        &self,
        session_id: u64,
        start: impl FnOnce() -> Result<(), String>,
        cancel: impl FnOnce(),
    ) -> Result<bool, String> {
        // Only hardware starts wait here; cancellation and Fn events never take this lock.
        let _capture = self.capture_start.lock().map_err(|e| e.to_string())?;
        if !self.is_starting(session_id) {
            return Ok(false);
        }
        let result = start();
        if !self.is_starting(session_id) {
            cancel();
            return Ok(false);
        }
        result.map(|_| true)
    }

    fn snapshot(&self) -> VoiceSnapshot {
        self.inner
            .lock()
            .expect("voice state poisoned")
            .snapshot
            .clone()
    }

    fn begin(&self, english: bool) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if !matches!(
            inner.snapshot.phase,
            VoicePhase::Idle | VoicePhase::Error | VoicePhase::Success
        ) {
            return None;
        }
        inner.snapshot.session_id = inner.snapshot.session_id.wrapping_add(1);
        inner.snapshot.phase = VoicePhase::Starting;
        inner.snapshot.status = "准备录音…".into();
        inner.snapshot.error = None;
        inner.snapshot.transcript = None;
        inner.snapshot.can_retry = false;
        inner.snapshot.can_insert = false;
        inner.snapshot.injected = false;
        inner.snapshot.elapsed_seconds = 0;
        inner.snapshot.limit_warning = false;
        inner.retained_pcm = None;
        inner.english = english;
        Some(inner.snapshot.clone())
    }

    fn english(&self, session_id: u64) -> Option<bool> {
        let inner = self.inner.lock().ok()?;
        (inner.snapshot.session_id == session_id).then_some(inner.english)
    }

    fn set_phase(
        &self,
        session_id: u64,
        phase: VoicePhase,
        status: impl Into<String>,
    ) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id {
            return None;
        }
        inner.snapshot.phase = phase;
        inner.snapshot.status = status.into();
        inner.snapshot.error = None;
        Some(inner.snapshot.clone())
    }

    fn fail(&self, session_id: u64, error: String) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id {
            return None;
        }
        inner.snapshot.phase = VoicePhase::Error;
        inner.snapshot.status = "输入失败".into();
        inner.snapshot.error = Some(error);
        inner.snapshot.can_retry = inner.retained_pcm.is_some();
        inner.snapshot.can_insert = false;
        Some(inner.snapshot.clone())
    }

    fn stop_recording(&self, session_id: u64) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id || inner.snapshot.phase != VoicePhase::Recording
        {
            return None;
        }
        inner.snapshot.phase = VoicePhase::Stopping;
        inner.snapshot.status = "正在结束录音…".into();
        Some(inner.snapshot.clone())
    }

    fn recording_elapsed(&self, session_id: u64, elapsed: u64) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id
            || inner.snapshot.phase != VoicePhase::Recording
            || inner.snapshot.elapsed_seconds == elapsed
        {
            return None;
        }
        inner.snapshot.elapsed_seconds = elapsed;
        inner.snapshot.limit_warning = elapsed >= RECORDING_WARNING_SECONDS;
        Some(inner.snapshot.clone())
    }

    fn retain_pcm(&self, session_id: u64, pcm: Arc<Vec<u8>>) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.snapshot.session_id != session_id {
            return false;
        }
        inner.retained_pcm = Some(pcm);
        true
    }

    fn is_current(&self, session_id: u64) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.snapshot.session_id == session_id)
            .unwrap_or(false)
    }

    fn finish_success(
        &self,
        session_id: u64,
        transcript: String,
        injected: bool,
        warning: Option<String>,
    ) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id {
            return None;
        }
        inner.snapshot.phase = VoicePhase::Success;
        inner.snapshot.status = warning.unwrap_or_else(|| {
            if injected {
                "文字已写入".into()
            } else {
                "文字已识别，请复制使用".into()
            }
        });
        inner.snapshot.error = None;
        inner.snapshot.can_retry = false;
        inner.snapshot.can_insert = !injected && !transcript.is_empty();
        inner.snapshot.transcript = Some(transcript);
        inner.snapshot.injected = injected;
        inner.retained_pcm = None;
        Some(inner.snapshot.clone())
    }

    fn cancel(&self) -> VoiceSnapshot {
        let mut inner = self.inner.lock().expect("voice state poisoned");
        if inner.snapshot.phase == VoicePhase::Idle && !inner.snapshot.can_insert {
            return inner.snapshot.clone();
        }
        // Incrementing invalidates permission and transcription futures immediately.
        inner.snapshot.session_id = inner.snapshot.session_id.wrapping_add(1);
        inner.snapshot.phase = VoicePhase::Idle;
        inner.snapshot.status.clear();
        inner.snapshot.error = None;
        inner.snapshot.transcript = None;
        inner.snapshot.can_retry = false;
        inner.snapshot.can_insert = false;
        inner.snapshot.injected = false;
        inner.snapshot.elapsed_seconds = 0;
        inner.snapshot.limit_warning = false;
        inner.retained_pcm = None;
        inner.snapshot.clone()
    }

    fn dismiss_success(&self, session_id: u64) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id || inner.snapshot.phase != VoicePhase::Success {
            return None;
        }
        inner.snapshot.phase = VoicePhase::Idle;
        Some(inner.snapshot.clone())
    }

    fn finish_without_speech(&self, session_id: u64) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id
            || inner.snapshot.phase != VoicePhase::Transcribing
        {
            return None;
        }
        inner.snapshot.phase = VoicePhase::Idle;
        inner.snapshot.status.clear();
        inner.snapshot.transcript = None;
        inner.snapshot.error = None;
        inner.snapshot.can_retry = false;
        inner.snapshot.can_insert = false;
        inner.retained_pcm = None;
        Some(inner.snapshot.clone())
    }

    fn begin_retry(&self) -> Option<(VoiceSnapshot, Arc<Vec<u8>>)> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.phase != VoicePhase::Error {
            return None;
        }
        let pcm = inner.retained_pcm.clone()?;
        inner.snapshot.session_id = inner.snapshot.session_id.wrapping_add(1);
        inner.snapshot.phase = VoicePhase::Retrying;
        inner.snapshot.status = "正在重试…".into();
        inner.snapshot.error = None;
        inner.snapshot.can_retry = false;
        Some((inner.snapshot.clone(), pcm))
    }

    fn begin_insertion(&self, session_id: u64, delayed: bool) -> Option<VoiceSnapshot> {
        let mut inner = self.inner.lock().ok()?;
        if inner.snapshot.session_id != session_id
            || !matches!(inner.snapshot.phase, VoicePhase::Success | VoicePhase::Idle)
            || !inner.snapshot.can_insert
            || inner.snapshot.injected
            || inner
                .snapshot
                .transcript
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return None;
        }
        // A new token invalidates queued clicks and the previous result's hide timer.
        inner.snapshot.session_id = inner.snapshot.session_id.wrapping_add(1);
        inner.snapshot.phase = VoicePhase::Inserting;
        inner.snapshot.status = if delayed {
            "请点击目标输入框，2 秒后填入".into()
        } else {
            "正在填入…".into()
        };
        inner.snapshot.can_insert = false;
        Some(inner.snapshot.clone())
    }
}

fn emit_snapshot(snapshot: &VoiceSnapshot) {
    crate::native::ui::update(serde_json::json!({"voice": snapshot}));
}

fn register_session_shortcuts(app: &tauri::AppHandle) {
    let shortcuts = app
        .state::<SettingsStore>()
        .settings
        .read()
        .map(|settings| {
            [
                settings.shortcut_cancel.clone(),
                settings.shortcut_finish.clone(),
            ]
        })
        .unwrap_or_else(|_| ["Esc".into(), "↵".into()]);
    for display in shortcuts {
        if let Ok(shortcut) = crate::settings::store::shortcut_to_tauri(&display) {
            if !app.global_shortcut().is_registered(shortcut.as_str()) {
                if let Err(error) = app.global_shortcut().register(shortcut.as_str()) {
                    log::warn!(
                        "unable to register session shortcut {}: {}",
                        shortcut,
                        error
                    );
                }
            }
        }
    }
}

fn unregister_session_shortcuts(app: &tauri::AppHandle) {
    let shortcuts = app
        .state::<SettingsStore>()
        .settings
        .read()
        .map(|settings| {
            [
                settings.shortcut_cancel.clone(),
                settings.shortcut_finish.clone(),
            ]
        })
        .unwrap_or_else(|_| ["Esc".into(), "↵".into()]);
    for display in shortcuts {
        if let Ok(shortcut) = crate::settings::store::shortcut_to_tauri(&display) {
            if app.global_shortcut().is_registered(shortcut.as_str()) {
                if let Err(error) = app.global_shortcut().unregister(shortcut.as_str()) {
                    log::warn!(
                        "unable to unregister session shortcut {}: {}",
                        shortcut,
                        error
                    );
                }
            }
        }
    }
}

fn matches_configured_shortcut(shortcut: &Shortcut, value: &str) -> bool {
    crate::settings::store::shortcut_to_tauri(value)
        .ok()
        .and_then(|value| value.parse::<Shortcut>().ok())
        .is_some_and(|configured| configured.id() == shortcut.id())
}

pub(crate) fn is_record_shortcut(app: &tauri::AppHandle, shortcut: &Shortcut) -> bool {
    app.state::<SettingsStore>()
        .settings
        .read()
        .map(|settings| matches_configured_shortcut(shortcut, &settings.shortcut_record))
        .unwrap_or(false)
}

pub(crate) fn is_cancel_shortcut(app: &tauri::AppHandle, shortcut: &Shortcut) -> bool {
    app.state::<SettingsStore>()
        .settings
        .read()
        .map(|settings| matches_configured_shortcut(shortcut, &settings.shortcut_cancel))
        .unwrap_or(false)
}

pub(crate) fn is_finish_shortcut(app: &tauri::AppHandle, shortcut: &Shortcut) -> bool {
    app.state::<SettingsStore>()
        .settings
        .read()
        .map(|settings| matches_configured_shortcut(shortcut, &settings.shortcut_finish))
        .unwrap_or(false)
}

fn set_phase(
    app: &tauri::AppHandle,
    session_id: u64,
    phase: VoicePhase,
    status: impl Into<String>,
) -> bool {
    let state = app.state::<VoiceSession>();
    let Some(snapshot) = state.set_phase(session_id, phase, status) else {
        return false;
    };
    emit_snapshot(&snapshot);
    true
}

fn fail(app: &tauri::AppHandle, session_id: u64, error: impl Into<String>) {
    let state = app.state::<VoiceSession>();
    if let Some(snapshot) = state.fail(session_id, error.into()) {
        emit_snapshot(&snapshot);
    }
}

pub async fn toggle(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    toggle_mode(app, false).await
}

pub async fn toggle_mode(app: tauri::AppHandle, english: bool) -> Result<VoiceSnapshot, String> {
    let snapshot = app.state::<VoiceSession>().snapshot();
    match snapshot.phase {
        VoicePhase::Idle | VoicePhase::Error | VoicePhase::Success => start(app, english).await,
        VoicePhase::Starting => cancel(app),
        VoicePhase::Recording => finish(app),
        _ => Ok(app.state::<VoiceSession>().snapshot()),
    }
}

async fn start(app: tauri::AppHandle, english: bool) -> Result<VoiceSnapshot, String> {
    Ok(start_session(&app, english).unwrap_or_else(|| app.state::<VoiceSession>().snapshot()))
}

fn start_session(app: &tauri::AppHandle, english: bool) -> Option<VoiceSnapshot> {
    if let Err(error) = crate::ai::local::validate_asr(app) {
        crate::native::ui::update(serde_json::json!({"notice": error}));
        return None;
    }
    let state = app.state::<VoiceSession>();
    let snapshot = state.with_setup(|| {
        let snapshot = state.begin(english)?;
        let session_id = snapshot.session_id;
        emit_snapshot(&snapshot);
        if !state.is_starting(session_id) {
            return None;
        }
        match crate::native::input_helper::begin_input_session(session_id) {
            Ok(true) => {}
            Ok(false) => {
                fail(app, session_id, "录音会话已失效，请重新开始");
                return None;
            }
            Err(error) => {
                fail(app, session_id, error);
                return None;
            }
        }
        register_session_shortcuts(app);
        Some(snapshot)
    })?;
    tauri::async_runtime::spawn(start_capture(app.clone(), snapshot.session_id));
    Some(snapshot)
}

async fn start_capture(app: tauri::AppHandle, session_id: u64) {
    if !app.state::<VoiceSession>().is_starting(session_id) {
        return;
    }
    if let Err(error) = crate::audio::commands::ensure_microphone_permission().await {
        fail_capture(&app, session_id, error);
        return;
    }
    if !app.state::<VoiceSession>().is_starting(session_id) {
        return;
    }
    let start_app = app.clone();
    let startup = async {
        let started = tauri::async_runtime::spawn_blocking(move || {
            let state = start_app.state::<VoiceSession>();
            state.start_current_capture(
                session_id,
                || {
                    let microphone_source = start_app
                        .state::<SettingsStore>()
                        .settings
                        .read()
                        .map_err(|error| error.to_string())?
                        .microphone_source
                        .clone();
                    start_app.state::<AudioRecorder>().start(&microphone_source)
                },
                || start_app.state::<AudioRecorder>().cancel(),
            )
        })
        .await
        .map_err(|error| format!("录音启动任务失败: {error}"))??;
        if !started {
            return Ok(false);
        }
        loop {
            if !app.state::<VoiceSession>().is_starting(session_id) {
                return Ok(false);
            }
            if let Some(error) = app.state::<AudioRecorder>().take_stream_error() {
                return Err(error);
            }
            if app.state::<AudioRecorder>().has_samples() {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    match tokio::time::timeout(MICROPHONE_START_TIMEOUT, startup).await {
        Ok(Ok(true)) => {
            let recording = app.state::<VoiceSession>().with_setup(|| {
                app.state::<VoiceSession>().is_starting(session_id)
                    && set_phase(&app, session_id, VoicePhase::Recording, "正在听…")
            });
            if recording {
                monitor_audio(app.clone(), session_id);
            }
        }
        Ok(Ok(false)) => {}
        Ok(Err(error)) => fail_capture(&app, session_id, error),
        Err(_) => fail_capture(
            &app,
            session_id,
            "麦克风启动超时，请检查所选输入设备后重试。".into(),
        ),
    }
}

fn fail_capture(app: &tauri::AppHandle, session_id: u64, error: String) {
    let state = app.state::<VoiceSession>();
    state.with_setup(|| {
        if state.is_starting(session_id) {
            fail(app, session_id, error);
            app.state::<AudioRecorder>().cancel();
        }
    });
}

pub(crate) async fn fn_press(app: tauri::AppHandle, english: bool) -> Result<Option<u64>, String> {
    let phase = app.state::<VoiceSession>().snapshot().phase;
    match phase {
        VoicePhase::Idle | VoicePhase::Error | VoicePhase::Success => {
            Ok(start_session(&app, english).map(|snapshot| snapshot.session_id))
        }
        VoicePhase::Starting => cancel(app).map(|_| None),
        VoicePhase::Recording => finish(app).map(|_| None),
        _ => Ok(None),
    }
}

pub(crate) fn fn_english(app: &tauri::AppHandle, session_id: u64) {
    app.state::<VoiceSession>().set_fn_english(session_id);
}

pub(crate) fn fn_cancel(app: tauri::AppHandle, session_id: u64) -> Result<(), String> {
    let state = app.state::<VoiceSession>();
    state.with_setup(|| {
        if state.fn_active(session_id) {
            cancel_setup(&app)?;
        }
        Ok(())
    })
}

fn monitor_audio(app: tauri::AppHandle, session_id: u64) {
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        let mut warned = false;

        loop {
            tokio::time::sleep(AUDIO_MONITOR_INTERVAL).await;
            let state = app.state::<VoiceSession>().snapshot();
            if state.session_id != session_id || state.phase != VoicePhase::Recording {
                break;
            }
            let elapsed = started.elapsed();
            if let Some(snapshot) = app
                .state::<VoiceSession>()
                .recording_elapsed(session_id, elapsed.as_secs())
            {
                emit_snapshot(&snapshot);
                if snapshot.limit_warning && !warned {
                    warned = true;
                    crate::native::ui::update(serde_json::json!({
                        "notice": "录音即将满 5 分钟，30 秒后会自动结束并识别。"
                    }));
                }
            }
            if elapsed >= MAX_RECORDING_DURATION {
                log::info!("voice session {}: five-minute limit reached", session_id);
                crate::native::ui::update(
                    serde_json::json!({"notice": "已录满 5 分钟，正在识别。"}),
                );
                let _ = finish_recording(app.clone(), session_id, None);
                break;
            }
            if let Some(error) = app.state::<AudioRecorder>().take_stream_error() {
                let _ = finish_recording(app.clone(), session_id, Some(error));
                break;
            }
            crate::native::ui::update(serde_json::json!({
                "audioLevels": app.state::<AudioRecorder>().levels(),
                "sessionId": session_id
            }));
        }
    });
}

fn finish(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    let snapshot = app.state::<VoiceSession>().snapshot();
    finish_recording(app, snapshot.session_id, None)
}

fn finish_recording(
    app: tauri::AppHandle,
    session_id: u64,
    interruption: Option<String>,
) -> Result<VoiceSnapshot, String> {
    let Some(snapshot) = app.state::<VoiceSession>().stop_recording(session_id) else {
        return Ok(app.state::<VoiceSession>().snapshot());
    };
    emit_snapshot(&snapshot);

    let pcm = match app.state::<AudioRecorder>().stop() {
        Ok(mut pcm) => {
            // The sidecar accepts at most five minutes; scheduler jitter must
            // not turn an otherwise valid maximum-length recording into an error.
            pcm.truncate(MAX_PCM_BYTES);
            Arc::new(pcm)
        }
        Err(error) => {
            fail(
                &app,
                session_id,
                interruption.map_or(error.clone(), |reason| {
                    format!("{reason}；{error}，请重新录音")
                }),
            );
            return Ok(app.state::<VoiceSession>().snapshot());
        }
    };
    if !app
        .state::<VoiceSession>()
        .retain_pcm(session_id, pcm.clone())
    {
        return Ok(app.state::<VoiceSession>().snapshot());
    }
    if let Some(error) = interruption {
        fail(
            &app,
            session_id,
            format!("{error}；已保留中断前的录音，可重试识别或重新录音"),
        );
    } else {
        tauri::async_runtime::spawn(process_pcm(app.clone(), session_id, pcm));
    }
    Ok(app.state::<VoiceSession>().snapshot())
}

async fn process_pcm(app: tauri::AppHandle, session_id: u64, pcm: Arc<Vec<u8>>) {
    let Some(english) = app.state::<VoiceSession>().english(session_id) else {
        return;
    };
    let language = match app.state::<SettingsStore>().settings.read() {
        Ok(values) => (values.stt_language != "auto").then(|| values.stt_language.clone()),
        Err(error) => {
            fail(&app, session_id, error.to_string());
            return;
        }
    };
    if !set_phase(&app, session_id, VoicePhase::Transcribing, "正在识别…") {
        return;
    }
    let result = tokio::select! {
        result = crate::ai::local::transcribe(&app, pcm.as_ref().clone(), language.as_deref(), session_id, english) => result,
        _ = wait_until_cancelled(&app, session_id) => {
            crate::ai::local::cancel_transcription(session_id).await;
            return;
        }
    };
    if !app.state::<VoiceSession>().is_current(session_id) {
        return;
    }
    let transcript = match result {
        Ok(Some(text)) => text,
        Ok(None) => {
            unregister_session_shortcuts(&app);
            if let Some(snapshot) = app
                .state::<VoiceSession>()
                .finish_without_speech(session_id)
            {
                emit_snapshot(&snapshot);
            }
            return;
        }
        Err(error) => {
            crate::ai::local::cancel_transcription(session_id).await;
            fail(&app, session_id, error);
            return;
        }
    };
    insert_transcript(
        app,
        session_id,
        transcript,
        Some(pcm.len() as f64 / 32000.0),
    )
    .await;
}

async fn insert_transcript(
    app: tauri::AppHandle,
    session_id: u64,
    transcript: String,
    duration: Option<f64>,
) {
    let mut insertion = 0;
    // The user may choose an input field in any app while speaking. Resolve
    // current focus only at writeback, with short retries for a warming AX tree.
    for delay in [0, 40, 80, 160] {
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        if !app.state::<VoiceSession>().is_current(session_id) {
            return;
        }
        insertion = crate::native::input_helper::insert_text(&transcript, session_id)
            .unwrap_or_else(|error| {
                log::warn!("voice session {session_id}: insertion failed: {error}");
                0
            });
        if insertion > 0 {
            break;
        }
    }
    let injected = insertion > 0;
    let status = if insertion == 1 {
        Some("已发送到输入框".to_string())
    } else if insertion <= 0 {
        log::warn!("voice session {session_id}: input refused, code {insertion}");
        Some(crate::native::input_helper::refusal_message(insertion).to_string())
    } else {
        None
    };
    let Some(snapshot) = app.state::<VoiceSession>().finish_success(
        session_id,
        transcript.clone(),
        injected,
        status,
    ) else {
        return;
    };
    if let Some(duration) = duration {
        save_session(&app, &snapshot, Some(duration));
    }
    if injected {
        crate::native::ui::record_first_input(&app);
    }
    emit_snapshot(&snapshot);
    schedule_success_hide(app.clone(), session_id);
}

async fn wait_until_cancelled(app: &tauri::AppHandle, session_id: u64) {
    while app.state::<VoiceSession>().is_current(session_id) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn save_session(app: &tauri::AppHandle, snapshot: &VoiceSnapshot, duration: Option<f64>) {
    use crate::database::{commands, Database};
    let Some(text) = snapshot.transcript.as_deref() else {
        return;
    };
    if let Err(error) = commands::create_record(app.state::<Database>(), text, duration) {
        log::error!("保存听写记录失败: {error}");
        crate::native::ui::update(
            serde_json::json!({"notice": "听写成功，但历史记录保存失败。请先复制结果。"}),
        );
    }
    let _ = app.emit("history-changed", ());
}

fn schedule_success_hide(app: tauri::AppHandle, session_id: u64) {
    let injected = app.state::<VoiceSession>().with_setup(|| {
        let snapshot = app.state::<VoiceSession>().snapshot();
        if snapshot.session_id != session_id || snapshot.phase != VoicePhase::Success {
            return None;
        }
        unregister_session_shortcuts(&app);
        Some(snapshot.injected)
    });
    let Some(injected) = injected else { return };
    let delay = if injected {
        Duration::from_millis(700)
    } else {
        Duration::from_secs(5)
    };
    let handle = app;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Some(snapshot) = handle.state::<VoiceSession>().dismiss_success(session_id) {
            emit_snapshot(&snapshot);
        }
    });
}

fn cancel(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    app.state::<VoiceSession>()
        .with_setup(|| cancel_setup(&app))
}

fn cancel_setup(app: &tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    let previous = app.state::<VoiceSession>().snapshot();
    if previous.phase == VoicePhase::Idle && !previous.can_insert {
        crate::native::ui::hide_hud();
        return Ok(previous);
    }
    log::info!(
        "voice session {} cancelled from {:?}",
        previous.session_id,
        previous.phase
    );
    let snapshot = app.state::<VoiceSession>().cancel();
    app.state::<AudioRecorder>().cancel();
    unregister_session_shortcuts(&app);
    crate::native::input_helper::clear_input_target();
    emit_snapshot(&snapshot);
    crate::native::ui::hide_hud();
    Ok(snapshot)
}

fn retry(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    let Some((snapshot, pcm)) = app.state::<VoiceSession>().begin_retry() else {
        return Ok(app.state::<VoiceSession>().snapshot());
    };
    emit_snapshot(&snapshot);
    let session_id = snapshot.session_id;
    crate::native::input_helper::begin_input_session(session_id)?;
    tauri::async_runtime::spawn(process_pcm(app.clone(), session_id, pcm));
    Ok(snapshot)
}

pub async fn voice_toggle(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    toggle(app).await
}

pub fn voice_finish(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    finish(app)
}

pub fn voice_cancel(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    cancel(app)
}

pub fn voice_retry(app: tauri::AppHandle) -> Result<VoiceSnapshot, String> {
    retry(app)
}

pub async fn retry_insertion(
    app: tauri::AppHandle,
    session_id: u64,
    delay_ms: u64,
) -> Result<VoiceSnapshot, String> {
    let delay = delay_ms.min(2000);
    let Some(snapshot) = app
        .state::<VoiceSession>()
        .begin_insertion(session_id, delay > 0)
    else {
        return Ok(app.state::<VoiceSession>().snapshot());
    };
    let session_id = snapshot.session_id;
    let transcript = snapshot.transcript.clone().unwrap_or_default();
    register_session_shortcuts(&app);
    emit_snapshot(&snapshot);
    if let Err(error) =
        crate::native::input_helper::begin_input_session(session_id).and_then(|accepted| {
            accepted
                .then_some(())
                .ok_or_else(|| "输入会话已过期".to_string())
        })
    {
        if let Some(snapshot) = app.state::<VoiceSession>().finish_success(
            session_id,
            transcript,
            false,
            Some(format!("暂时无法填入，请重试或复制文字：{error}")),
        ) {
            emit_snapshot(&snapshot);
            schedule_success_hide(app.clone(), session_id);
        }
        return Ok(app.state::<VoiceSession>().snapshot());
    }
    if delay > 0 {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
            _ = wait_until_cancelled(&app, session_id) => return Ok(app.state::<VoiceSession>().snapshot()),
        }
    }
    insert_transcript(app.clone(), session_id, transcript, None).await;
    Ok(app.state::<VoiceSession>().snapshot())
}

pub fn voice_get_state(state: tauri::State<'_, VoiceSession>) -> VoiceSnapshot {
    state.snapshot()
}

pub(crate) fn cancel_from_shortcut(app: tauri::AppHandle) {
    let _ = cancel(app);
}

pub(crate) fn finish_from_shortcut(app: tauri::AppHandle) {
    let snapshot = app.state::<VoiceSession>().snapshot();
    if snapshot.phase == VoicePhase::Error {
        if snapshot.can_retry {
            let _ = retry(app);
        } else {
            tauri::async_runtime::spawn(async move {
                let _ = start(app, false).await;
            });
        }
    } else {
        let _ = finish(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_or_timed_out_start_cleans_up_before_a_new_session_can_open() {
        for timed_out in [false, true] {
            use std::sync::{
                atomic::{AtomicU64, Ordering},
                mpsc,
            };
            let state = Arc::new(VoiceSession::new());
            let first = state.begin(false).unwrap();
            let resource = Arc::new(AtomicU64::new(0));
            let (entered_tx, entered_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let old_state = state.clone();
            let old_resource = resource.clone();
            let old = std::thread::spawn(move || {
                old_state.start_current_capture(
                    first.session_id,
                    || {
                        entered_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                        old_resource.store(first.session_id, Ordering::SeqCst);
                        Ok(())
                    },
                    || old_resource.store(0, Ordering::SeqCst),
                )
            });
            entered_rx.recv().unwrap();
            if timed_out {
                state.fail(first.session_id, "timeout".into()).unwrap();
            } else {
                state.cancel();
            }
            let next = state.begin(false).unwrap();
            let new_state = state.clone();
            let new_resource = resource.clone();
            let (waiting_tx, waiting_rx) = mpsc::channel();
            let new = std::thread::spawn(move || {
                waiting_tx
                    .send(new_state.capture_start.try_lock().is_err())
                    .unwrap();
                new_state.start_current_capture(
                    next.session_id,
                    || {
                        new_resource.store(next.session_id, Ordering::SeqCst);
                        Ok(())
                    },
                    || new_resource.store(0, Ordering::SeqCst),
                )
            });
            assert!(waiting_rx.recv().unwrap());
            resume_tx.send(()).unwrap();
            assert!(!old.join().unwrap().unwrap());
            assert!(new.join().unwrap().unwrap());
            assert_eq!(resource.load(Ordering::SeqCst), next.session_id);
            assert!(!state.is_starting(first.session_id));
            assert!(state.is_starting(next.session_id));
        }
    }

    #[test]
    fn fn_shift_keeps_english_for_capture_and_cannot_change_a_later_session() {
        let state = VoiceSession::new();
        let first = state.begin(false).unwrap();
        assert!(state.set_fn_english(first.session_id));
        // Fn release drops only the worker's token; the session keeps its output mode.
        state.set_phase(first.session_id, VoicePhase::Recording, "first PCM");
        assert_eq!(state.english(first.session_id), Some(true));
        state.cancel();
        let next = state.begin(false).unwrap();
        assert!(!state.fn_active(first.session_id));
        assert!(!state.set_fn_english(first.session_id));
        assert_eq!(state.english(next.session_id), Some(false));
        state.set_phase(next.session_id, VoicePhase::Transcribing, "识别中");
        assert!(!state.fn_active(next.session_id));
        assert!(!state.set_fn_english(next.session_id));
    }

    #[test]
    fn cancellation_cleans_registration_when_requested_during_setup() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        let state = Arc::new(VoiceSession::new());
        let registered = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let start_state = state.clone();
        let start_registered = registered.clone();
        let starting = std::thread::spawn(move || {
            start_state.with_setup(|| {
                start_state.begin(false).unwrap();
                entered_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                start_registered.store(true, Ordering::SeqCst);
            })
        });
        entered_rx.recv().unwrap();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let cancel_state = state.clone();
        let cancel_registered = registered.clone();
        let cancelling = std::thread::spawn(move || {
            attempt_tx
                .send(cancel_state.setup.try_lock().is_err())
                .unwrap();
            cancel_state.with_setup(|| {
                cancel_state.cancel();
                cancel_registered.store(false, Ordering::SeqCst);
            });
        });
        assert!(attempt_rx.recv().unwrap());
        resume_tx.send(()).unwrap();
        starting.join().unwrap();
        cancelling.join().unwrap();
        assert_eq!(state.snapshot().phase, VoicePhase::Idle);
        assert!(!registered.load(Ordering::SeqCst));
    }

    #[test]
    fn interrupted_recording_retains_audio_and_can_only_stop_once() {
        let state = VoiceSession::new();
        let first = state.begin(true).unwrap();
        state.set_phase(first.session_id, VoicePhase::Recording, "正在听…");
        assert!(state.stop_recording(first.session_id).is_some());
        assert!(state.stop_recording(first.session_id).is_none());
        let pcm = Arc::new(vec![1, 2, 3, 4]);
        assert!(state.retain_pcm(first.session_id, pcm.clone()));
        let failed = state.fail(first.session_id, "麦克风中断".into()).unwrap();
        assert!(failed.can_retry);
        assert!(!failed.can_insert);
        let (retried, retained) = state.begin_retry().unwrap();
        assert!(Arc::ptr_eq(&pcm, &retained));
        assert_eq!(state.english(retried.session_id), Some(true));
        assert!(state.stop_recording(first.session_id).is_none());
        assert!(!state.retain_pcm(first.session_id, pcm));
    }

    #[test]
    fn reinsertion_claim_rejects_double_clicks_expired_timers_and_late_completions() {
        let state = VoiceSession::new();
        let first = state.begin(false).unwrap();
        assert!(state.begin_insertion(first.session_id, false).is_none());
        state
            .finish_success(first.session_id, "待填入文字".into(), false, None)
            .unwrap();
        let hidden = state.dismiss_success(first.session_id).unwrap();
        assert!(hidden.can_insert);
        let claimed = state.begin_insertion(first.session_id, true).unwrap();
        assert_eq!(claimed.phase, VoicePhase::Inserting);
        assert!(!claimed.can_insert);
        assert_eq!(claimed.transcript.as_deref(), Some("待填入文字"));
        assert!(state.begin_insertion(first.session_id, false).is_none());
        assert!(state.begin_insertion(claimed.session_id, false).is_none());
        assert!(state.dismiss_success(first.session_id).is_none());
        assert!(state.begin(false).is_none());
        state.cancel();
        assert!(state
            .finish_success(claimed.session_id, "旧文字".into(), true, None)
            .is_none());
        assert!(!state.snapshot().can_insert);
    }

    #[test]
    fn reinsertion_is_reusable_after_refusal_but_not_after_injection_or_cancel() {
        let state = VoiceSession::new();
        let first = state.begin(false).unwrap();
        state
            .finish_success(first.session_id, "文字".into(), false, None)
            .unwrap();
        let claimed = state.begin_insertion(first.session_id, false).unwrap();
        state
            .finish_success(claimed.session_id, "文字".into(), false, None)
            .unwrap();
        let second = state.begin_insertion(claimed.session_id, false).unwrap();
        state
            .finish_success(second.session_id, "文字".into(), true, None)
            .unwrap();
        assert!(!state.snapshot().can_insert);
        assert!(state.begin_insertion(second.session_id, false).is_none());
        let next = state.begin(false).unwrap();
        state
            .finish_success(next.session_id, "文字".into(), false, None)
            .unwrap();
        state.dismiss_success(next.session_id).unwrap();
        let cancelled = state.cancel();
        assert_ne!(cancelled.session_id, next.session_id);
        assert!(state.begin_insertion(next.session_id, false).is_none());
    }

    #[test]
    fn elapsed_updates_warn_at_four_minutes_thirty_and_ignore_old_sessions() {
        let state = VoiceSession::new();
        let first = state.begin(false).unwrap();
        assert!(state.recording_elapsed(first.session_id, 269).is_none());
        state.set_phase(first.session_id, VoicePhase::Recording, "正在听…");
        assert!(
            !state
                .recording_elapsed(first.session_id, 269)
                .unwrap()
                .limit_warning
        );
        let warning = state.recording_elapsed(first.session_id, 270).unwrap();
        assert!(warning.limit_warning);
        assert_eq!(warning.elapsed_seconds, 270);
        assert!(state.recording_elapsed(first.session_id, 270).is_none());
        state.cancel();
        let next = state.begin(false).unwrap();
        assert!(state.recording_elapsed(first.session_id, 300).is_none());
        assert_eq!(next.elapsed_seconds, 0);
        assert!(!next.limit_warning);
    }

    #[test]
    fn result_timeout_preserves_text_and_cannot_close_a_new_recording() {
        let state = VoiceSession::new();
        let first = state.begin(false).unwrap();
        state
            .finish_success(first.session_id, "003821".into(), false, None)
            .unwrap();
        let hidden = state.dismiss_success(first.session_id).unwrap();
        assert_eq!(hidden.phase, VoicePhase::Idle);
        assert_eq!(hidden.transcript.as_deref(), Some("003821"));
        let next = state.begin(false).unwrap();
        assert!(state.dismiss_success(first.session_id).is_none());
        assert_eq!(state.snapshot().phase, VoicePhase::Starting);
        assert!(state.dismiss_success(next.session_id).is_none());
    }

    #[test]
    fn no_speech_ends_quietly_and_cannot_finish_a_new_session() {
        let state = VoiceSession::new();
        let first = state.begin(false).unwrap();
        state.retain_pcm(first.session_id, Arc::new(vec![1, 2]));
        state.set_phase(first.session_id, VoicePhase::Transcribing, "正在识别…");
        let empty = state.finish_without_speech(first.session_id).unwrap();
        assert_eq!(empty.phase, VoicePhase::Idle);
        assert!(empty.transcript.is_none() && empty.error.is_none() && !empty.can_retry);
        assert!(state.inner.lock().unwrap().retained_pcm.is_none());
        let next = state.begin(false).unwrap();
        assert!(state.finish_without_speech(first.session_id).is_none());
        assert!(state.finish_without_speech(next.session_id).is_none());
        assert_eq!(state.snapshot().phase, VoicePhase::Starting);
    }

    #[test]
    fn cancel_invalidates_an_active_session() {
        let state = VoiceSession::new();
        let started = state.begin(false).unwrap();
        let cancelled = state.cancel();
        assert_eq!(cancelled.phase, VoicePhase::Idle);
        assert_ne!(cancelled.session_id, started.session_id);
        assert!(!state.is_current(started.session_id));

        let cancelled_again = state.cancel();
        assert_eq!(cancelled_again.session_id, cancelled.session_id);
    }

    #[test]
    fn cancelled_session_cannot_publish_a_late_result() {
        let state = VoiceSession::new();
        let started = state.begin(false).unwrap();
        state.cancel();
        assert!(state
            .set_phase(started.session_id, VoicePhase::Recording, "late microphone")
            .is_none());
        assert!(state
            .fail(started.session_id, "late startup failure".into())
            .is_none());
        assert!(state
            .finish_success(started.session_id, "stale".into(), true, None)
            .is_none());
    }

    #[test]
    fn recording_limit_is_five_minutes() {
        assert_eq!(MAX_RECORDING_DURATION, Duration::from_secs(300));
    }

    #[test]
    fn output_mode_survives_retry_and_resets_for_new_recording() {
        let state = VoiceSession::new();
        let started = state.begin(true).unwrap();
        state.retain_pcm(started.session_id, Arc::new(vec![1, 2]));
        state.fail(started.session_id, "retry".into());
        let (retried, _) = state.begin_retry().unwrap();
        assert_eq!(state.english(retried.session_id), Some(true));
        assert_eq!(state.english(started.session_id), None);
        state.cancel();
        let next = state.begin(false).unwrap();
        assert_eq!(state.english(next.session_id), Some(false));
    }
}
