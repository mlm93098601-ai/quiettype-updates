use crate::native::util_helper;
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

struct CaptureContext {
    recording: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
}

pub(super) struct VoiceProcessingStream {
    handle: *mut c_void,
    _context: Box<CaptureContext>,
}

// Swift serializes stop against its tap callback. The boxed context stays at
// a stable address until the engine and every callback have stopped.
unsafe impl Send for VoiceProcessingStream {}

impl VoiceProcessingStream {
    pub fn start(
        recording: Arc<AtomicBool>,
        buffer: Arc<Mutex<Vec<f32>>>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Result<(Self, u32), String> {
        let mut context = Box::new(CaptureContext {
            recording,
            buffer,
            error,
        });
        let mut sample_rate = 0.0;
        let handle = unsafe {
            util_helper::start_voice_capture(
                receive_samples,
                (&mut *context as *mut CaptureContext).cast(),
                &mut sample_rate,
            )?
        };
        Ok((
            Self {
                handle,
                _context: context,
            },
            sample_rate as u32,
        ))
    }
}

unsafe extern "C" fn receive_samples(data: *const f32, count: u32, context: *mut c_void) {
    let context = &*context.cast::<CaptureContext>();
    if !context.recording.load(Ordering::Relaxed) {
        return;
    }
    if data.is_null() {
        if let Ok(mut error) = context.error.lock() {
            *error = Some("麦克风或扬声器连接已变化，请重新开始录音".into());
        }
    } else if let Ok(mut buffer) = context.buffer.lock() {
        buffer.extend_from_slice(std::slice::from_raw_parts(data, count as usize));
    }
}

impl Drop for VoiceProcessingStream {
    fn drop(&mut self) {
        unsafe { util_helper::stop_voice_capture(self.handle) };
    }
}
