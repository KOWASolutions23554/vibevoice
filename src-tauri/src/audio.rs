use crate::hotkey::now_ms;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use hound::{SampleFormat as WavSampleFormat, WavSpec, WavWriter};
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
}

#[derive(Debug, Clone)]
pub struct RecordingResult {
    pub wav_data: Vec<u8>,
    pub duration_ms: u32,
    pub rms: f32,
}

pub(crate) const MIN_SPEECH_DURATION_MS: u32 = 450;
pub(crate) const MIN_SPEECH_RMS: f32 = 0.007;

// A healthy WASAPI capture stream delivers data every ~10ms even in silence.
// No data for this long means the stream died (device removed, sleep/resume, ...).
const STREAM_STALE_MS: u64 = 1_500;
// Tighter bound at stop time: the stream was verified healthy at start, so any
// larger gap since the last delivered data means it died mid-recording.
const STOP_STALE_MS: u64 = 400;

fn default_device_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|device| device.name().ok())
}

impl RecordingResult {
    pub fn has_speech(&self) -> bool {
        self.duration_ms >= MIN_SPEECH_DURATION_MS && self.rms >= MIN_SPEECH_RMS
    }
}

struct Recorder {
    samples: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
    recording: Arc<AtomicBool>,
    sample_rate: u32,
    stream_error: Arc<AtomicBool>,
    last_data_ms: Arc<AtomicU64>,
    device_name: Option<String>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            recording: Arc::new(AtomicBool::new(false)),
            sample_rate: 16_000,
            stream_error: Arc::new(AtomicBool::new(false)),
            last_data_ms: Arc::new(AtomicU64::new(0)),
            device_name: None,
        }
    }

    fn prepare_stream(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No microphone found".to_string())?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;

        self.sample_rate = config.sample_rate().0;
        let stream_config: StreamConfig = config.clone().into();
        let samples = Arc::clone(&self.samples);
        let recording = Arc::clone(&self.recording);
        let channel_count = config.channels() as usize;

        let error_flag = Arc::clone(&self.stream_error);
        let err_fn = move |err| {
            eprintln!("Audio stream error: {err}");
            error_flag.store(true, Ordering::SeqCst);
        };

        let heartbeat = Arc::clone(&self.last_data_ms);

        let stream = match config.sample_format() {
            SampleFormat::I16 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        if recording.load(Ordering::SeqCst) {
                            append_samples(&samples, data, channel_count);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?,
            SampleFormat::F32 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        if recording.load(Ordering::SeqCst) {
                            let converted: Vec<i16> = data
                                .iter()
                                .map(|&sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                                .collect();
                            append_samples(&samples, &converted, channel_count);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?,
            SampleFormat::U16 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &_| {
                        heartbeat.store(now_ms(), Ordering::SeqCst);
                        if recording.load(Ordering::SeqCst) {
                            let converted: Vec<i16> = data
                                .iter()
                                .map(|&sample| sample as i32 - i16::MAX as i32)
                                .map(|sample| sample as i16)
                                .collect();
                            append_samples(&samples, &converted, channel_count);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?,
            _ => return Err("Unsupported audio sample format".to_string()),
        };

        stream.play().map_err(|e| e.to_string())?;
        self.device_name = device.name().ok();
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
        // Follow the Windows default device: if the user switched microphones,
        // the old stream still runs but captures the wrong device.
        self.device_name.is_some() && self.device_name == default_device_name()
    }

    fn start_recording(&mut self) -> Result<(), String> {
        if self.recording.load(Ordering::SeqCst) {
            return Ok(());
        }

        if !self.stream_is_healthy() {
            self.stream = None;
            self.prepare_stream()?;
        }
        self.samples.lock().unwrap().clear();
        self.recording.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop_recording(&mut self) -> Result<RecordingResult, String> {
        let was_recording = self.recording.swap(false, Ordering::SeqCst);

        let samples = self.samples.lock().unwrap().clone();
        let (duration_ms, rms) = analyze_samples(&samples, self.sample_rate);

        // The stream errored or stopped delivering data mid-recording.
        // Rebuild it now so the next attempt works.
        let stream_died = was_recording
            && (self.stream_error.load(Ordering::SeqCst)
                || now_ms().saturating_sub(self.last_data_ms.load(Ordering::SeqCst))
                    > STOP_STALE_MS);
        if stream_died {
            self.stream = None;
            let _ = self.prepare_stream();
            return Err("Microphone connection lost — reconnected. Please try again.".to_string());
        }

        let wav_data = encode_wav(&samples, self.sample_rate)?;

        Ok(RecordingResult {
            wav_data,
            duration_ms,
            rms,
        })
    }
}

pub struct AudioHandle {
    command_tx: Sender<AudioCommand>,
}

impl AudioHandle {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();

        thread::spawn(move || {
            let mut recorder = Recorder::new();
            if let Err(error) = recorder.prepare_stream() {
                eprintln!("Audio prewarm failed: {error}");
            }
            while let Ok(command) = command_rx.recv() {
                match command {
                    AudioCommand::Start { reply } => {
                        let _ = reply.send(recorder.start_recording());
                    }
                    AudioCommand::Stop { reply } => {
                        let _ = reply.send(recorder.stop_recording());
                    }
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

fn analyze_samples(samples: &[i16], sample_rate: u32) -> (u32, f32) {
    if samples.is_empty() || sample_rate == 0 {
        return (0, 0.0);
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
    (duration_ms, rms)
}

fn encode_wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>, String> {
    let mut cursor = Cursor::new(Vec::new());
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: WavSampleFormat::Int,
    };

    let mut writer = WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
    for &sample in samples {
        writer.write_sample(sample).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;

    Ok(cursor.into_inner())
}
