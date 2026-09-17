use crate::hotkey::now_ms;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use hound::{SampleFormat as WavSampleFormat, WavSpec, WavWriter};
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

enum AudioCommand {
    Start {
        reply: Sender<Result<(), String>>,
    },
    Stop {
        reply: Sender<Result<RecordingResult, String>>,
    },
    SetDevice {
        device_name: String,
    },
    ListDevices {
        reply: Sender<Vec<String>>,
    },
}

#[derive(Debug, Clone)]
pub struct RecordingResult {
    pub wav_data: Vec<u8>,
    pub duration_ms: u32,
    pub rms: f32,
    pub peak_window_rms: f32,
}

pub(crate) const MIN_SPEECH_DURATION_MS: u32 = 250;
pub(crate) const MIN_SPEECH_RMS: f32 = 0.0015;
pub(crate) const MIN_PEAK_WINDOW_RMS: f32 = 0.0035;

// Trailing-silence trimming: Whisper hallucinates phantom words during the
// silent tail a recording picks up after the last spoken word.
const SILENCE_WINDOW_MS: u32 = 25;
const TRAILING_SILENCE_PAD_MS: u32 = 150;

// If a stream delivers no data for 1.5s while idle, it needs reconnection.
const STREAM_STALE_MS: u64 = 1_500;
// Stop stale tolerance: allow plenty of room beyond the 400ms hotkey debounce.
const STOP_STALE_MS: u64 = 2_000;

impl RecordingResult {
    pub fn has_speech(&self) -> bool {
        self.duration_ms >= MIN_SPEECH_DURATION_MS
            && (self.rms >= MIN_SPEECH_RMS || self.peak_window_rms >= MIN_PEAK_WINDOW_RMS)
    }
}

pub fn safe_device_name(device: &cpal::Device) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| device.name().ok()))
        .ok()
        .flatten()
}

pub fn safe_default_device() -> Option<cpal::Device> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cpal::default_host().default_input_device()
    }))
    .ok()
    .flatten()
}

pub fn safe_input_devices() -> Vec<cpal::Device> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cpal::default_host()
            .input_devices()
            .map(|iter| iter.collect::<Vec<_>>())
            .unwrap_or_default()
    }))
    .unwrap_or_default()
}

pub fn list_input_devices() -> Vec<String> {
    let mut names = Vec::new();
    for dev in safe_input_devices() {
        if let Some(name) = safe_device_name(&dev) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn current_default_device_name() -> Option<String> {
    safe_default_device().and_then(|dev| safe_device_name(&dev))
}

struct Recorder {
    samples: Arc<Mutex<Vec<i16>>>,
    preroll: Arc<Mutex<VecDeque<i16>>>,
    stream: Option<cpal::Stream>,
    recording: Arc<AtomicBool>,
    sample_rate: u32,
    stream_error: Arc<AtomicBool>,
    last_data_ms: Arc<AtomicU64>,
    device_name: Option<String>,
    target_device: String,
}

impl Recorder {
    fn new(target_device: String) -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            preroll: Arc::new(Mutex::new(VecDeque::new())),
            stream: None,
            recording: Arc::new(AtomicBool::new(false)),
            sample_rate: 16_000,
            stream_error: Arc::new(AtomicBool::new(false)),
            last_data_ms: Arc::new(AtomicU64::new(0)),
            device_name: None,
            target_device,
        }
    }

    fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }

    fn set_target_device(&mut self, device_name: String) {
        if self.target_device != device_name {
            self.target_device = device_name;
            self.stream = None;
            let _ = self.prepare_stream();
        }
    }

    fn find_device(&self) -> Result<cpal::Device, String> {
        let target = self.target_device.trim();
        let use_default = target.is_empty() || target.eq_ignore_ascii_case("default");

        if !use_default {
            for dev in safe_input_devices() {
                if let Some(name) = safe_device_name(&dev) {
                    if name == target {
                        return Ok(dev);
                    }
                }
            }
        }

        // Try Windows default input device
        if let Some(dev) = safe_default_device() {
            return Ok(dev);
        }

        // Fallback to first available active input device
        let all = safe_input_devices();
        if let Some(first) = all.into_iter().next() {
            return Ok(first);
        }

        Err("No microphone found".to_string())
    }

    fn prepare_stream(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }

        let device = self.find_device()?;
        let dev_name = safe_device_name(&device);

        let config = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            device.default_input_config()
        }))
        .map_err(|_| "Failed to query microphone configuration".to_string())?
        .map_err(|e| e.to_string())?;

        self.sample_rate = config.sample_rate().0;
        let stream_config: StreamConfig = config.clone().into();
        let samples = Arc::clone(&self.samples);
        let preroll = Arc::clone(&self.preroll);
        let recording = Arc::clone(&self.recording);
        let channel_count = config.channels() as usize;

        // Keep ~250ms pre-roll to prevent initial consonant clipping
        let max_preroll = (self.sample_rate as usize * 250 / 1000).max(1);

        let error_flag = Arc::clone(&self.stream_error);
        let err_fn = move |err| {
            eprintln!("Audio stream error: {err}");
            error_flag.store(true, Ordering::SeqCst);
        };

        let heartbeat = Arc::clone(&self.last_data_ms);

        let stream_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match config.sample_format() {
                SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, data, channel_count);
                        } else {
                            append_preroll(&preroll, data, channel_count, max_preroll);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I32 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i32], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        let converted: Vec<i16> =
                            data.iter().map(|&sample| (sample >> 16) as i16).collect();
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, &converted, channel_count);
                        } else {
                            append_preroll(&preroll, &converted, channel_count, max_preroll);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        let converted: Vec<i16> = data
                            .iter()
                            .map(|&sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                            .collect();
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, &converted, channel_count);
                        } else {
                            append_preroll(&preroll, &converted, channel_count, max_preroll);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::U16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        let converted: Vec<i16> = data
                            .iter()
                            .map(|&sample| sample as i32 - i16::MAX as i32)
                            .map(|sample| sample as i16)
                            .collect();
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, &converted, channel_count);
                        } else {
                            append_preroll(&preroll, &converted, channel_count, max_preroll);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::U8 => device.build_input_stream(
                    &stream_config,
                    move |data: &[u8], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        let converted: Vec<i16> = data
                            .iter()
                            .map(|&sample| ((sample as i32 - 128) << 8) as i16)
                            .collect();
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, &converted, channel_count);
                        } else {
                            append_preroll(&preroll, &converted, channel_count, max_preroll);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I8 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i8], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        let converted: Vec<i16> =
                            data.iter().map(|&sample| (sample as i16) << 8).collect();
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, &converted, channel_count);
                        } else {
                            append_preroll(&preroll, &converted, channel_count, max_preroll);
                        }
                    },
                    err_fn,
                    None,
                ),
                _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
            }
        }));

        let stream = match stream_res {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err("Microphone stream initialization failed".to_string()),
        };

        let play_res =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stream.play()));
        match play_res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err("Failed to start microphone stream".to_string()),
        }

        self.device_name = dev_name;
        self.stream_error.store(false, Ordering::SeqCst);
        self.last_data_ms.store(now_ms(), Ordering::SeqCst);
        self.stream = Some(stream);
        Ok(())
    }

    fn stream_is_healthy(&self) -> bool {
        if self.stream.is_none() {
            return false;
        }
        if self.stream_error.load(Ordering::SeqCst) {
            return false;
        }
        let last_data = self.last_data_ms.load(Ordering::SeqCst);
        if now_ms().saturating_sub(last_data) > STREAM_STALE_MS {
            return false;
        }

        let target = self.target_device.trim();
        let use_default = target.is_empty() || target.eq_ignore_ascii_case("default");
        if use_default {
            if let Some(def_name) = current_default_device_name() {
                if self.device_name.as_ref() != Some(&def_name) {
                    return false;
                }
            }
        } else if self.device_name.as_ref() != Some(&self.target_device) {
            return false;
        }

        true
    }

    fn maintain_stream(&mut self) {
        if !self.stream_is_healthy() {
            self.stream = None;
            let _ = self.prepare_stream();
        }
    }

    fn start_recording(&mut self) -> Result<(), String> {
        if self.recording.load(Ordering::SeqCst) {
            return Ok(());
        }

        if !self.stream_is_healthy() {
            self.stream = None;
            let _ = self.prepare_stream();
        }

        if self.stream.is_none() {
            self.prepare_stream()?;
        }

        // Transfer pre-roll buffer to prevent clipped speech on quick push-to-talk
        let preroll_samples: Vec<i16> = {
            let mut preroll = self.preroll.lock().unwrap();
            preroll.drain(..).collect()
        };

        let mut samples = self.samples.lock().unwrap();
        samples.clear();
        samples.extend(preroll_samples);

        self.recording.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop_recording(&mut self) -> Result<RecordingResult, String> {
        let was_recording = self.recording.swap(false, Ordering::SeqCst);

        let samples = trim_trailing_silence(&self.samples.lock().unwrap(), self.sample_rate);
        let (duration_ms, rms, peak_window_rms) = analyze_samples(&samples, self.sample_rate);

        let last_gap = now_ms().saturating_sub(self.last_data_ms.load(Ordering::SeqCst));
        let stream_had_issues = was_recording
            && (self.stream_error.load(Ordering::SeqCst) || last_gap > STOP_STALE_MS);

        if stream_had_issues {
            self.stream = None;
            let _ = self.prepare_stream();
        }

        let result = RecordingResult {
            wav_data: encode_wav(&samples, self.sample_rate)?,
            duration_ms,
            rms,
            peak_window_rms,
        };

        // If speech was recorded, NEVER discard it!
        if result.has_speech() {
            return Ok(result);
        }

        if stream_had_issues {
            return Err("Microphone connection lost — reconnected. Please try again.".to_string());
        }

        Ok(result)
    }
}

pub struct AudioHandle {
    command_tx: Sender<AudioCommand>,
}

impl AudioHandle {
    pub fn spawn(initial_device: String) -> Self {
        let (command_tx, command_rx) = mpsc::channel();

        thread::spawn(move || {
            #[cfg(windows)]
            unsafe {
                use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }

            let mut recorder = Recorder::new(initial_device);
            if let Err(error) = recorder.prepare_stream() {
                eprintln!("Audio prewarm failed: {error}");
            }

            loop {
                match command_rx.recv_timeout(std::time::Duration::from_millis(800)) {
                    Ok(AudioCommand::Start { reply }) => {
                        let _ = reply.send(recorder.start_recording());
                    }
                    Ok(AudioCommand::Stop { reply }) => {
                        let _ = reply.send(recorder.stop_recording());
                    }
                    Ok(AudioCommand::SetDevice { device_name }) => {
                        recorder.set_target_device(device_name);
                    }
                    Ok(AudioCommand::ListDevices { reply }) => {
                        let _ = reply.send(list_input_devices());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if !recorder.is_recording() {
                            recorder.maintain_stream();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Self { command_tx }
    }

    pub fn start_recording(&self) -> Result<(), String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.command_tx
            .send(AudioCommand::Start { reply: reply_tx })
            .map_err(|_| "Audio thread stopped".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "Audio thread stopped".to_string())?
    }

    pub fn stop_recording(&self) -> Result<RecordingResult, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.command_tx
            .send(AudioCommand::Stop { reply: reply_tx })
            .map_err(|_| "Audio thread stopped".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "Audio thread stopped".to_string())?
    }

    pub fn set_device(&self, device_name: &str) {
        let _ = self.command_tx.send(AudioCommand::SetDevice {
            device_name: device_name.to_string(),
        });
    }

    pub fn list_devices(&self) -> Vec<String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .command_tx
            .send(AudioCommand::ListDevices { reply: reply_tx })
            .is_ok()
        {
            if let Ok(devices) = reply_rx.recv_timeout(std::time::Duration::from_millis(500)) {
                return devices;
            }
        }
        list_input_devices()
    }
}

fn append_samples(samples: &Arc<Mutex<Vec<i16>>>, data: &[i16], channels: usize) {
    let mut buffer = samples.lock().unwrap();
    if channels <= 1 {
        buffer.extend_from_slice(data);
        return;
    }

    for frame in data.chunks(channels) {
        let sum: i32 = frame.iter().map(|&sample| sample as i32).sum();
        buffer.push((sum / channels as i32) as i16);
    }
}

fn append_preroll(
    preroll: &Arc<Mutex<VecDeque<i16>>>,
    data: &[i16],
    channels: usize,
    max_samples: usize,
) {
    let mut buffer = preroll.lock().unwrap();
    if channels <= 1 {
        buffer.extend(data.iter().copied());
    } else {
        for frame in data.chunks(channels) {
            let sum: i32 = frame.iter().map(|&sample| sample as i32).sum();
            buffer.push_back((sum / channels as i32) as i16);
        }
    }
    while buffer.len() > max_samples {
        buffer.pop_front();
    }
}

fn window_rms(chunk: &[i16]) -> f32 {
    if chunk.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = chunk
        .iter()
        .map(|&sample| {
            let normalized = sample as f64 / i16::MAX as f64;
            normalized * normalized
        })
        .sum();
    (sum_sq / chunk.len() as f64).sqrt() as f32
}

// Cut the silent tail of a recording so the clip ends shortly after the last
// spoken word. A short pad is kept so soft word endings are not clipped.
fn trim_trailing_silence(samples: &[i16], sample_rate: u32) -> Vec<i16> {
    let window = sample_rate as usize * SILENCE_WINDOW_MS as usize / 1000;
    if samples.is_empty() || sample_rate == 0 || samples.len() <= window {
        return samples.to_vec();
    }

    let mut end = samples.len();
    while end > window {
        if window_rms(&samples[end - window..end]) >= MIN_SPEECH_RMS {
            break;
        }
        end -= window;
    }

    let pad = sample_rate as usize * TRAILING_SILENCE_PAD_MS as usize / 1000;
    let end = (end + pad).min(samples.len());
    samples[..end].to_vec()
}

fn analyze_samples(samples: &[i16], sample_rate: u32) -> (u32, f32, f32) {
    if samples.is_empty() || sample_rate == 0 {
        return (0, 0.0, 0.0);
    }

    let duration_ms = (samples.len() as u64 * 1000 / sample_rate as u64) as u32;
    let sum_sq: f64 = samples
        .iter()
        .map(|&sample| {
            let normalized = sample as f64 / i16::MAX as f64;
            normalized * normalized
        })
        .sum();

    let rms = (sum_sq / samples.len() as f64).sqrt() as f32;

    // Windowed peak RMS across 200ms chunks to detect speech even if surrounded by silence
    let window_size = (sample_rate as usize / 5).max(1);
    let mut peak_window_rms: f32 = 0.0;
    for chunk in samples.chunks(window_size) {
        if chunk.len() < window_size / 2 {
            continue;
        }
        let chunk_sum: f64 = chunk
            .iter()
            .map(|&sample| {
                let normalized = sample as f64 / i16::MAX as f64;
                normalized * normalized
            })
            .sum();
        let chunk_rms = (chunk_sum / chunk.len() as f64).sqrt() as f32;
        if chunk_rms > peak_window_rms {
            peak_window_rms = chunk_rms;
        }
    }

    (duration_ms, rms, peak_window_rms)
}

fn resample_to_16k(samples: &[i16], sample_rate: u32) -> Vec<i16> {
    if sample_rate == 16_000 || samples.is_empty() || sample_rate == 0 {
        return samples.to_vec();
    }
    let target_rate = 16_000u64;
    let new_len = (samples.len() as u64 * target_rate / sample_rate as u64) as usize;
    if new_len == 0 {
        return Vec::new();
    }
    let mut resampled = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_idx = (i as u64 * sample_rate as u64) as f64 / target_rate as f64;
        let idx0 = src_idx.floor() as usize;
        let frac = (src_idx - idx0 as f64) as f32;
        let idx1 = (idx0 + 1).min(samples.len() - 1);
        let s0 = samples[idx0] as f32;
        let s1 = samples[idx1] as f32;
        resampled.push((s0 + frac * (s1 - s0)).round() as i16);
    }
    resampled
}

fn encode_wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>, String> {
    let resampled = resample_to_16k(samples, sample_rate);
    let mut cursor = Cursor::new(Vec::new());
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: WavSampleFormat::Int,
    };

    let mut writer = WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
    for &sample in &resampled {
        writer.write_sample(sample).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;

    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_48k_to_16k() {
        let input_48k = vec![1000i16; 4800]; // 100ms of 48kHz
        let output_16k = resample_to_16k(&input_48k, 48_000);
        assert_eq!(output_16k.len(), 1600); // 100ms of 16kHz
        assert_eq!(output_16k[500], 1000);
    }

    #[test]
    fn detects_speech_with_peak_window_despite_trailing_silence() {
        // 1 second of audio at 16kHz: 300ms speech (RMS ~0.02) + 700ms silence (RMS 0)
        let mut samples = Vec::new();
        for _ in 0..4800 {
            samples.push(650i16); // ~0.02 normalized
        }
        for _ in 0..11200 {
            samples.push(0i16);
        }

        let (duration, rms, peak_window) = analyze_samples(&samples, 16_000);
        assert_eq!(duration, 1000);
        assert!(peak_window >= 0.01);

        let result = RecordingResult {
            wav_data: Vec::new(),
            duration_ms: duration,
            rms,
            peak_window_rms: peak_window,
        };
        assert!(result.has_speech());
    }

    #[test]
    fn rejects_pure_silence() {
        let samples = vec![0i16; 16000];
        let (duration, rms, peak_window) = analyze_samples(&samples, 16_000);
        let result = RecordingResult {
            wav_data: Vec::new(),
            duration_ms: duration,
            rms,
            peak_window_rms: peak_window,
        };
        assert!(!result.has_speech());
    }

    #[test]
    fn trims_trailing_silence() {
        // 300ms speech + 700ms silence -> clip ends 150ms after the speech
        let mut samples = Vec::new();
        for _ in 0..4800 {
            samples.push(650i16); // ~0.02 normalized
        }
        for _ in 0..11200 {
            samples.push(0i16);
        }

        let trimmed = trim_trailing_silence(&samples, 16_000);
        assert_eq!(trimmed.len(), 7200);
        assert!(trimmed[..4800].iter().all(|&s| s == 650));

        let (duration, rms, peak_window) = analyze_samples(&trimmed, 16_000);
        let result = RecordingResult {
            wav_data: Vec::new(),
            duration_ms: duration,
            rms,
            peak_window_rms: peak_window,
        };
        assert!(result.has_speech());
    }

    #[test]
    fn keeps_clip_without_trailing_silence() {
        let samples = vec![650i16; 16_000];
        let trimmed = trim_trailing_silence(&samples, 16_000);
        assert_eq!(trimmed.len(), samples.len());
    }

    #[test]
    fn trims_pure_silence_to_a_sliver() {
        let samples = vec![0i16; 16_000];
        let trimmed = trim_trailing_silence(&samples, 16_000);
        assert!(trimmed.len() < 16_000 / 2);
    }
}
