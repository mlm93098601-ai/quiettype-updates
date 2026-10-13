pub mod commands;
mod voice_processing;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// 原生音频录制器：普通听写用 cpal，仅通信应用音频活跃时使用回声消除。
/// 由 Rust 控制用户选择的麦克风，不改变系统默认设备。
/// 麦克风只在 start() 时激活，stop() 后立即释放。
pub struct AudioRecorder {
    inner: Mutex<RecorderInner>,
    recording: Arc<AtomicBool>,
    starting: AtomicBool,
}

struct RecorderInner {
    voice_stream: Option<voice_processing::VoiceProcessingStream>,
    /// cpal 音频流（持有时麦克风活跃，Drop 后释放）
    stream: Option<cpal::Stream>,
    /// 原始 f32 PCM 缓冲（单声道，原始采样率）
    buffer: Arc<Mutex<Vec<f32>>>,
    /// 设备原始采样率
    sample_rate: u32,
    /// 音频设备断开或路由切换时由 cpal 回调写入。
    stream_error: Arc<Mutex<Option<String>>>,
}

// cpal::Stream 在 macOS 上是 Send，Mutex 让整个结构变成 Send+Sync
unsafe impl Send for RecorderInner {}

impl AudioRecorder {
    pub fn new() -> Self {
        let recording = Arc::new(AtomicBool::new(false));
        Self {
            recording,
            starting: AtomicBool::new(false),
            inner: Mutex::new(RecorderInner {
                voice_stream: None,
                stream: None,
                buffer: Arc::new(Mutex::new(Vec::new())),
                sample_rate: 48000,
                stream_error: Arc::new(Mutex::new(None)),
            }),
        }
    }

    /// 只在用户开始录音时打开选定输入设备。
    pub fn start(&self, microphone_source: &str) -> Result<(), String> {
        if self.starting.swap(true, Ordering::SeqCst) {
            return Err("录音正在启动中".to_string());
        }
        if self.recording.swap(true, Ordering::SeqCst) {
            self.starting.store(false, Ordering::SeqCst);
            return Err("正在录音中".to_string());
        }
        let result = (|| {
            let (buffer, stream_error) = {
                let inner = self.inner.lock().map_err(|e| e.to_string())?;
                inner.buffer.lock().map_err(|e| e.to_string())?.clear();
                *inner.stream_error.lock().map_err(|e| e.to_string())? = None;
                (inner.buffer.clone(), inner.stream_error.clone())
            };
            let (device, uses_system_input) = selected_input_device(microphone_source)?;
            let needs_processing = uses_system_input
                && crate::native::util_helper::communication_audio_active().unwrap_or_else(
                    |error| {
                        log::warn!("通话音频检测不可用，使用普通录音: {error}");
                        false
                    },
                );
            if needs_processing {
                match voice_processing::VoiceProcessingStream::start(
                    self.recording.clone(),
                    buffer,
                    stream_error,
                ) {
                    Ok((stream, sample_rate)) => {
                        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
                        if !self.recording.load(Ordering::SeqCst) {
                            return Err("录音已取消".into());
                        }
                        inner.sample_rate = sample_rate;
                        inner.voice_stream = Some(stream);
                        log::info!("录音已开始: 系统回声消除, {}Hz, mono", sample_rate);
                        return Ok(());
                    }
                    Err(error) => {
                        // ponytail: unusual device pairs may not support VoiceProcessingIO;
                        // preserve dictation with raw input rather than muting a call.
                        log::warn!("回声消除不可用，使用原始麦克风: {}", error);
                    }
                }
            }
            log::info!(
                "录音设备: {}",
                device.name().unwrap_or_else(|_| "unknown".into())
            );

            let config = device
                .default_input_config()
                .map_err(|e| format!("获取音频配置失败: {}", e))?;
            let sample_rate = config.sample_rate().0;
            let channels = config.channels() as usize;
            let sample_format = config.sample_format();
            log::info!(
                "录音参数: {}Hz, {}ch, {:?}",
                sample_rate,
                channels,
                sample_format
            );

            let (buffer, stream_error) = {
                let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
                inner.sample_rate = sample_rate;
                inner.buffer.lock().unwrap().clear();
                if let Ok(mut error) = inner.stream_error.lock() {
                    *error = None;
                }
                (inner.buffer.clone(), inner.stream_error.clone())
            };
            let recording = self.recording.clone();
            let err_fn = move |err: cpal::StreamError| {
                log::error!("音频流错误: {}", err);
                if let Ok(mut current) = stream_error.lock() {
                    *current = Some(format!("麦克风连接中断: {}", err));
                }
            };

            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if !recording.load(Ordering::Relaxed) {
                            return;
                        }
                        let mono = downmix_to_mono_f32(data, channels);
                        if let Ok(mut buf) = buffer.lock() {
                            buf.extend_from_slice(&mono);
                        }
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => {
                    let buffer = buffer.clone();
                    let recording = recording.clone();
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if !recording.load(Ordering::Relaxed) {
                                return;
                            }
                            let float_data: Vec<f32> =
                                data.iter().map(|&s| s as f32 / 32768.0).collect();
                            let mono = downmix_to_mono_f32(&float_data, channels);
                            if let Ok(mut buf) = buffer.lock() {
                                buf.extend_from_slice(&mono);
                            }
                        },
                        err_fn,
                        None,
                    )
                }
                cpal::SampleFormat::U16 => {
                    let buffer = buffer.clone();
                    let recording = recording.clone();
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            if !recording.load(Ordering::Relaxed) {
                                return;
                            }
                            let float_data: Vec<f32> = data
                                .iter()
                                .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                .collect();
                            let mono = downmix_to_mono_f32(&float_data, channels);
                            if let Ok(mut buf) = buffer.lock() {
                                buf.extend_from_slice(&mono);
                            }
                        },
                        err_fn,
                        None,
                    )
                }
                fmt => return Err(format!("不支持的音频格式: {:?}", fmt)),
            }
            .map_err(|e| format!("创建音频流失败: {}", e))?;

            stream.play().map_err(|e| format!("启动录音失败: {}", e))?;
            let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
            if !self.recording.load(Ordering::SeqCst) {
                return Err("录音已取消".to_string());
            }
            inner.stream = Some(stream);
            log::info!("录音已开始");
            Ok(())
        })();
        if result.is_err() {
            self.recording.store(false, Ordering::SeqCst);
        }
        self.starting.store(false, Ordering::SeqCst);
        result
    }

    pub fn has_samples(&self) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.buffer.lock().ok().map(|buffer| !buffer.is_empty()))
            .unwrap_or(false)
    }

    pub fn levels(&self) -> [f32; 11] {
        let Ok(inner) = self.inner.lock() else {
            return [0.0; 11];
        };
        let Ok(buffer) = inner.buffer.lock() else {
            return [0.0; 11];
        };
        // Spread the eleven bars across 420 ms so neighbouring bars retain
        // different parts of a syllable instead of the same short vowel.
        // This is only a visual meter; recognition audio remains unchanged.
        let window = (inner.sample_rate as usize / 50).max(1);
        let hop = (inner.sample_rate as usize / 25).max(1);
        let details: [f32; 11] = std::array::from_fn(|index| {
            let end = buffer.len().saturating_sub((10 - index) * hop);
            let samples = &buffer[end.saturating_sub(window)..end];
            let rms = compute_rms(samples);
            rms + 0.24 * (compute_peak(samples) - rms).max(0.0)
        });
        let peak = details.into_iter().fold(0.0f32, f32::max);
        let strength = meter_level(peak);
        // Normalize before the compressed volume curve: ordinary/loud speech
        // must preserve bar-to-bar contrast even when absolute volume is high.
        details.map(|detail| {
            if peak <= 0.0001 {
                0.0
            } else {
                strength * (detail / peak).clamp(0.0, 1.0).powf(1.6)
            }
        })
    }

    /// 停止录音，返回 16kHz/16bit/mono 的原始 PCM 字节（无 WAV 头）。
    pub fn stop(&self) -> Result<Vec<u8>, String> {
        if !self.recording.swap(false, Ordering::SeqCst) {
            return Err("没有正在进行的录音".to_string());
        }

        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        // Drop stream → 释放麦克风
        inner.stream.take();
        inner.voice_stream.take();

        let mut samples = {
            let mut buf = inner.buffer.lock().map_err(|e| e.to_string())?;
            std::mem::take(&mut *buf)
        };

        if samples.is_empty() {
            return Err("录音数据为空".to_string());
        }

        let duration_secs = samples.len() as f32 / inner.sample_rate as f32;
        log::info!(
            "录音已停止: {} 采样点, {:.2}s, 原始 {}Hz",
            samples.len(),
            duration_secs,
            inner.sample_rate
        );

        // Keep rejecting accidental taps, but allow short commands such as
        // “好” or “确认”.
        if duration_secs < 0.15 {
            return Err("录音时间太短（需要至少 0.15 秒）".to_string());
        }

        // Sanitize the stream, remove DC offset and measure the actual signal.
        // CoreAudio may keep delivering zero-filled buffers when microphone
        // access is missing, which previously looked like a valid recording.
        for sample in &mut samples {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }
        let dc_offset = samples.iter().sum::<f32>() / samples.len() as f32;
        for sample in &mut samples {
            *sample -= dc_offset;
        }

        let input_rms = compute_rms(&samples);
        let input_peak = compute_peak(&samples);
        log::info!(
            "录音信号: rms={:.6}, peak={:.6}, dc={:.6}",
            input_rms,
            input_peak,
            dc_offset
        );

        if input_rms < 0.00001 && input_peak < 0.0001 {
            return Err(
                "麦克风采集到的是静音。请检查 macOS 麦克风权限和系统默认输入设备。".to_string(),
            );
        }

        // Low amplitude does not mean unclear speech. Normalize first; the local
        // VAD then distinguishes speech from amplified background noise.
        // ponytail: at most 64x gain; VAD rejects noise but does not denoise speech.
        let gain_for_rms = 0.08 / input_rms.max(f32::EPSILON);
        let gain_for_peak = 0.92 / input_peak.max(f32::EPSILON);
        let gain = gain_for_rms.min(gain_for_peak).min(64.0).max(1.0);
        if gain > 1.05 {
            for sample in &mut samples {
                *sample = (*sample * gain).clamp(-1.0, 1.0);
            }
            log::info!(
                "已增强麦克风音量: gain={:.2}x, rms={:.6}, peak={:.6}",
                gain,
                compute_rms(&samples),
                compute_peak(&samples)
            );
        }

        // 重采样到 16kHz
        let target_rate = 16000u32;
        let resampled = if inner.sample_rate == target_rate {
            samples
        } else {
            resample(&samples, inner.sample_rate, target_rate)
        };

        // f32 → i16 PCM 字节（little-endian，无 WAV 头）
        let mut pcm_bytes = Vec::with_capacity(resampled.len() * 2);
        for &sample in &resampled {
            let s = sample.clamp(-1.0, 1.0);
            let i16_val = if s < 0.0 {
                (s * 32768.0) as i16
            } else {
                (s * 32767.0) as i16
            };
            pcm_bytes.extend_from_slice(&i16_val.to_le_bytes());
        }

        log::info!(
            "PCM 输出: {} 字节 ({} 采样点 @ 16kHz, {:.2}s)",
            pcm_bytes.len(),
            resampled.len(),
            resampled.len() as f32 / 16000.0
        );
        Ok(pcm_bytes)
    }

    /// Stop immediately and discard buffered audio. Safe to call repeatedly.
    pub fn cancel(&self) {
        self.recording.store(false, Ordering::SeqCst);
        if let Ok(mut inner) = self.inner.lock() {
            inner.stream.take();
            inner.voice_stream.take();
            if let Ok(mut buffer) = inner.buffer.lock() {
                buffer.clear();
            }
        }
    }

    pub fn take_stream_error(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.stream_error.lock().ok()?.take())
    }
}

pub fn validate_microphone_source(source: &str) -> Result<(), String> {
    selected_input_device(source).map(|_| ())
}

fn selected_input_device(source: &str) -> Result<(cpal::Device, bool), String> {
    let host = cpal::default_host();
    if source == "system" {
        return host
            .default_input_device()
            .map(|device| (device, true))
            .ok_or_else(|| "未检测到系统默认麦克风".into());
    }
    let devices = crate::native::util_helper::microphone_devices()?;
    let selected = choose_microphone(source, &devices)?;
    if selected.default_input {
        let device = host
            .default_input_device()
            .ok_or("未检测到系统默认麦克风")?;
        if !device.name().is_ok_and(|name| name == selected.name) {
            return Err("系统默认麦克风已变化，请重试".into());
        }
        return Ok((device, true));
    }
    let name = &selected.name;
    let mut matches = host
        .devices()
        .map_err(|e| format!("无法读取麦克风设备：{e}"))?
        .filter(|device| device.name().is_ok_and(|value| value == *name))
        .filter(|device| {
            device
                .supported_input_configs()
                .is_ok_and(|mut configs| configs.next().is_some())
        });
    let device = matches.next().ok_or("所选麦克风已断开，请重新连接后重试")?;
    if matches.next().is_some() {
        return Err("有多个同名麦克风，无法安全选择，请断开其他同名设备后重试".into());
    }
    Ok((device, false))
}

fn choose_microphone<'a>(
    source: &str,
    devices: &'a [crate::native::util_helper::MicrophoneDevice],
) -> Result<&'a crate::native::util_helper::MicrophoneDevice, String> {
    let candidates: Vec<_> = devices
        .iter()
        .filter(|device| match source {
            "builtIn" => device.built_in,
            "headset" => device.headset,
            _ => false,
        })
        .collect();
    if candidates.is_empty() {
        return Err(match source {
            "builtIn" => "未检测到电脑内置麦克风",
            "headset" => "未检测到可用的耳机麦克风，请连接耳机后重试",
            _ => "不支持的麦克风来源",
        }
        .into());
    }
    if let Some(device) = candidates
        .iter()
        .find(|device| device.default_input)
        .or_else(|| candidates.iter().find(|device| device.default_output))
    {
        return Ok(device);
    }
    if candidates.len() != 1 {
        return Err("检测到多个可用麦克风，无法安全选择，请仅连接要使用的设备后重试".into());
    }
    Ok(candidates[0])
}

/// 多声道下混为单声道
fn downmix_to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }

    // Pick the channel carrying the strongest signal instead of averaging all
    // channels. Averaging can erase speech when a virtual/aggregate device has
    // silent or phase-inverted channels.
    let mut best_channel = 0usize;
    let mut best_energy = f64::NEG_INFINITY;
    for channel in 0..channels {
        let mut energy = 0.0f64;
        for sample in data.iter().skip(channel).step_by(channels) {
            energy += (*sample as f64) * (*sample as f64);
        }
        if energy > best_energy {
            best_energy = energy;
            best_channel = channel;
        }
    }

    data.chunks_exact(channels)
        .map(|frame| frame[best_channel])
        .collect()
}

/// 计算 RMS 音量
fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

fn meter_level(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    // Lift quiet syllables for the tiny HUD while retaining headroom and a
    // stationary noise floor. This curve affects display only, never ASR audio.
    ((20.0 * rms.log10() + 80.0) / 58.0)
        .clamp(0.0, 1.0)
        .powf(0.65)
}

fn compute_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max)
}

/// 线性插值重采样（足够用于语音识别）
fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (input.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = (src_pos - src_idx as f64) as f32;

        let s0 = input.get(src_idx).copied().unwrap_or(0.0);
        let s1 = input.get(src_idx + 1).copied().unwrap_or(s0);
        output.push(s0 + (s1 - s0) * frac);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_selection_uses_kind_and_defaults_without_falling_back() {
        use crate::native::util_helper::MicrophoneDevice;
        let devices = vec![
            MicrophoneDevice {
                name: "电脑麦克风".into(),
                built_in: true,
                headset: false,
                default_input: true,
                default_output: false,
            },
            MicrophoneDevice {
                name: "已连接耳机".into(),
                built_in: false,
                headset: true,
                default_input: false,
                default_output: true,
            },
            MicrophoneDevice {
                name: "USB 摄像头".into(),
                built_in: false,
                headset: false,
                default_input: false,
                default_output: false,
            },
        ];
        assert_eq!(
            choose_microphone("builtIn", &devices).unwrap().name,
            "电脑麦克风"
        );
        assert!(
            choose_microphone("builtIn", &devices)
                .unwrap()
                .default_input
        );
        assert_eq!(
            choose_microphone("headset", &devices).unwrap().name,
            "已连接耳机"
        );
        assert!(
            !choose_microphone("headset", &devices)
                .unwrap()
                .default_input
        );
        assert!(choose_microphone("headset", &devices[..1]).is_err());
        assert!(choose_microphone("unknown", &devices).is_err());
        let mut ambiguous = vec![
            MicrophoneDevice {
                name: "耳机一".into(),
                built_in: false,
                headset: true,
                default_input: false,
                default_output: false,
            },
            MicrophoneDevice {
                name: "耳机二".into(),
                built_in: false,
                headset: true,
                default_input: false,
                default_output: false,
            },
        ];
        assert!(choose_microphone("headset", &ambiguous).is_err());
        ambiguous[1].default_output = true;
        assert_eq!(
            choose_microphone("headset", &ambiguous).unwrap().name,
            "耳机二"
        );
        ambiguous[0].default_input = true;
        assert_eq!(
            choose_microphone("headset", &ambiguous).unwrap().name,
            "耳机一"
        );
        ambiguous[0].name = "同名耳机".into();
        ambiguous[1].name = "同名耳机".into();
        let selected = choose_microphone("headset", &ambiguous).unwrap();
        assert!(std::ptr::eq(selected, &ambiguous[0]));
        assert!(selected.default_input);
    }

    #[test]
    fn interrupted_audio_is_stopped_and_preserved_instead_of_cancelled() {
        let recorder = AudioRecorder::new();
        recorder.recording.store(true, Ordering::SeqCst);
        let mut inner = recorder.inner.lock().unwrap();
        inner.sample_rate = 16000;
        *inner.buffer.lock().unwrap() = (0..8000)
            .map(|i| 0.2 * (i as f32 * std::f32::consts::TAU * 440.0 / 16000.0).sin())
            .collect();
        *inner.stream_error.lock().unwrap() = Some("麦克风连接中断".into());
        drop(inner);
        assert!(recorder.take_stream_error().is_some());
        let pcm = recorder.stop().unwrap();
        assert_eq!(pcm.len(), 16000);
        assert!(pcm.iter().any(|byte| *byte != 0));
        assert!(!recorder.recording.load(Ordering::SeqCst));
        let inner = recorder.inner.lock().unwrap();
        assert!(inner.stream.is_none() && inner.voice_stream.is_none());
        assert!(inner.buffer.lock().unwrap().is_empty());
    }

    #[test]
    fn meter_retains_quiet_speech_and_time_variation() {
        assert_eq!(meter_level(0.0), 0.0);
        assert_eq!(meter_level(f32::NAN), 0.0);
        assert_eq!(meter_level(0.0001), 0.0);
        assert!(meter_level(0.0005) > 0.35);
        let recorder = AudioRecorder::new();
        let mut inner = recorder.inner.lock().unwrap();
        inner.sample_rate = 16000;
        let amplitudes = [0.0, 0.0005, 0.003, 0.02, 0.2];
        *inner.buffer.lock().unwrap() = amplitudes
            .into_iter()
            .flat_map(|amplitude| std::iter::repeat_n(amplitude, 400))
            .collect();
        drop(inner);
        let levels = recorder.levels();
        assert_eq!(levels[0], 0.0);
        assert!(levels[9] > 0.01);
        assert!(levels[6] < 0.95);
        assert_eq!(levels[10], 1.0);
        assert!(levels.windows(2).all(|pair| pair[0] <= pair[1]));
        let amplitudes = [
            0.08, 0.2, 0.06, 0.15, 0.04, 0.2, 0.07, 0.12, 0.05, 0.18, 0.09,
        ];
        *recorder.inner.lock().unwrap().buffer.lock().unwrap() = amplitudes
            .into_iter()
            .flat_map(|amplitude| std::iter::repeat_n(amplitude, 640))
            .collect();
        let levels = recorder.levels();
        assert!(
            levels
                .windows(2)
                .filter(|pair| (pair[0] - pair[1]).abs() > 0.25)
                .count()
                >= 8,
            "Loud syllables must retain clearly staggered neighbouring bars"
        );
    }

    #[test]
    fn quiet_speech_is_amplified_without_clipping_or_accepting_silence() {
        let stop_signal = |amplitude: f32| {
            let recorder = AudioRecorder::new();
            recorder.recording.store(true, Ordering::SeqCst);
            let mut inner = recorder.inner.lock().unwrap();
            inner.sample_rate = 16000;
            *inner.buffer.lock().unwrap() = (0..16000)
                .map(|i| amplitude * (i as f32 * std::f32::consts::TAU * 440.0 / 16000.0).sin())
                .collect();
            drop(inner);
            recorder.stop()
        };

        let quiet_pcm = stop_signal(0.001).expect("Quiet speech must reach the recognizer");
        let quiet: Vec<f32> = quiet_pcm
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0)
            .collect();
        assert_eq!(quiet.len(), 16000);
        assert!(compute_rms(&quiet) > 0.04);
        assert!(compute_peak(&quiet) < 0.1);

        let loud_pcm = stop_signal(0.95).unwrap();
        let loud_peak = loud_pcm
            .chunks_exact(2)
            .map(|bytes| (i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0).abs())
            .fold(0.0f32, f32::max);
        assert!((loud_peak - 0.95).abs() < 0.001);
        assert!(stop_signal(0.0).is_err());
    }

    #[test]
    fn downmix_uses_the_channel_with_speech() {
        let stereo = [0.0, 0.4, 0.0, -0.4, 0.0, 0.2];
        assert_eq!(downmix_to_mono_f32(&stereo, 2), vec![0.4, -0.4, 0.2]);
    }

    #[test]
    fn downmix_does_not_cancel_phase_inverted_channels() {
        let stereo = [0.5, -0.5, -0.25, 0.25];
        let mono = downmix_to_mono_f32(&stereo, 2);
        assert!(compute_peak(&mono) > 0.49);
    }

    #[test]
    fn cancellation_discards_audio_and_releases_the_session() {
        let recorder = AudioRecorder::new();
        recorder.recording.store(true, Ordering::SeqCst);
        recorder
            .inner
            .lock()
            .unwrap()
            .buffer
            .lock()
            .unwrap()
            .extend([0.25, -0.25]);
        recorder.cancel();
        recorder.cancel();
        assert!(!recorder.recording.load(Ordering::SeqCst));
        assert!(recorder
            .inner
            .lock()
            .unwrap()
            .buffer
            .lock()
            .unwrap()
            .is_empty());
    }
}
