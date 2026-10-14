//! Opens the output device and hands the engine to its callback.

use crate::engine::{Cmd, Engine, Garbage};
use crate::prefs::Prefs;
use crate::shared::Shared;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;

pub struct AudioHandle {
    pub shared: Arc<Shared>,
    pub tx: Producer<Cmd>,
    pub tx_midi: Producer<Cmd>,
    pub garbage: Consumer<Garbage>,
    /// Dropping the stream stops audio, so the app keeps it alive.
    pub stream: Option<cpal::Stream>,
    pub error: Option<String>,
    /// A preference that could not be honoured, and what was used instead.
    pub note: Option<String>,
    pub device_name: String,
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, mut engine: Engine) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    // Used only when the device doesn't take f32 directly; sized once, up front.
    let mut scratch = vec![0f32; 1 << 16];
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for chunk in data.chunks_mut(scratch.len() / channels * channels) {
                    let buf = &mut scratch[..chunk.len()];
                    engine.process(buf, channels);
                    for (o, s) in chunk.iter_mut().zip(buf.iter()) {
                        *o = T::from_sample(*s);
                    }
                }
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|e| e.to_string())
}

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.to_string()).unwrap_or_else(|_| "output".into())
}

/// An output device and the common sample rates it accepts.
pub struct OutputInfo {
    pub name: String,
    pub rates: Vec<u32>,
}

const COMMON_RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

pub fn output_devices() -> Vec<OutputInfo> {
    let Ok(devices) = cpal::default_host().output_devices() else { return Vec::new() };
    devices
        .map(|d| {
            let ranges: Vec<_> = d.supported_output_configs().map(|c| c.collect()).unwrap_or_default();
            let rates = COMMON_RATES
                .into_iter()
                .filter(|r| ranges.iter().any(|c: &cpal::SupportedStreamConfigRange| c.min_sample_rate() <= *r && *r <= c.max_sample_rate()))
                .collect();
            OutputInfo { name: device_name(&d), rates }
        })
        .collect()
}

/// Picks the device and stream settings the preferences ask for, falling back to the
/// system default for anything unavailable. The second value says what was substituted.
fn choose_output(prefs: &Prefs) -> Result<(cpal::Device, cpal::SupportedStreamConfig, Option<String>), String> {
    let host = cpal::default_host();
    let mut note = None;
    let wanted = prefs.output_device.as_ref().and_then(|name| {
        let found = host.output_devices().ok()?.find(|d| &device_name(d) == name);
        if found.is_none() {
            note = Some(format!("{name} not found; using the default output"));
        }
        found
    });
    let device = wanted.or_else(|| host.default_output_device()).ok_or("no audio output device found")?;
    let default = device.default_output_config().map_err(|e| e.to_string())?;
    let config = match prefs.sample_rate {
        Some(rate) if rate != default.sample_rate() => {
            // Same channel count and format as the default, at the requested rate.
            let matched = device.supported_output_configs().ok().and_then(|mut configs| {
                configs
                    .find(|c| c.channels() == default.channels() && c.sample_format() == default.sample_format() && c.try_with_sample_rate(rate).is_some())
                    .and_then(|c| c.try_with_sample_rate(rate))
            });
            if matched.is_none() {
                note = Some(format!("{rate} Hz is not available on this device; using {} Hz", default.sample_rate()));
            }
            matched.unwrap_or(default)
        }
        _ => default,
    };
    Ok((device, config, note))
}

/// The sample rate the preferences would run at, if the device can be opened.
pub fn planned_rate(prefs: &Prefs) -> Option<u32> {
    choose_output(prefs).ok().map(|(_, config, _)| config.sample_rate())
}

pub fn start(prefs: &Prefs) -> AudioHandle {
    let (tx, rx) = RingBuffer::new(8192);
    let (tx_midi, rx_midi) = RingBuffer::new(1024);
    let (gtx, grx) = RingBuffer::new(4096);
    let skip_secs = prefs.skipback_minutes as usize * 60;

    let (device, supported, mut note) = match choose_output(prefs) {
        Ok(v) => v,
        Err(e) => {
            // Keep the app usable for editing even with no device.
            return AudioHandle {
                shared: Arc::new(Shared::new(48000, skip_secs)),
                tx,
                tx_midi,
                garbage: grx,
                stream: None,
                error: Some(e),
                note: None,
                device_name: "none".into(),
            };
        }
    };
    let format = supported.sample_format();
    let buffer_range = *supported.buffer_size();
    let mut config: cpal::StreamConfig = supported.into();
    if let Some(frames) = prefs.buffer_frames {
        // Only ask for a size the driver says it can do; a refused size fails the whole stream.
        match buffer_range {
            cpal::SupportedBufferSize::Range { min, max } if (min..=max).contains(&frames) => {
                config.buffer_size = cpal::BufferSize::Fixed(frames);
            }
            _ => note = Some(format!("a {frames}-frame buffer is not available on this device; using its default")),
        }
    }
    let shared = Arc::new(Shared::new(config.sample_rate, skip_secs));
    let engine = Engine::new(shared.clone(), rx, rx_midi, gtx);
    let device_name = device_name(&device);

    use cpal::SampleFormat as F;
    let stream = match format {
        F::F32 => build::<f32>(&device, config, engine),
        F::I16 => build::<i16>(&device, config, engine),
        F::U16 => build::<u16>(&device, config, engine),
        F::I32 => build::<i32>(&device, config, engine),
        other => Err(format!("unsupported device sample format {other}")),
    }
    .and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string()));

    let (stream, error) = match stream {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e)),
    };
    AudioHandle { shared, tx, tx_midi, garbage: grx, stream, error, note, device_name }
}

/// Names of the available capture devices, in the order `open_input` indexes them.
pub fn input_names() -> Vec<String> {
    let Ok(devices) = cpal::default_host().input_devices() else { return Vec::new() };
    devices
        .map(|d| device_name(&d))
        .collect()
}

pub struct InputHandle {
    pub stream: cpal::Stream,
    /// Interleaved stereo frames from the capture thread.
    pub rx: Consumer<f32>,
    /// Device rate / engine rate; the engine resamples by this.
    pub ratio: f32,
}

fn build_input<T>(device: &cpal::Device, config: cpal::StreamConfig, mut tx: Producer<f32>) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                for frame in data.chunks_exact(channels) {
                    let l = f32::from_sample(frame[0]);
                    let r = if channels > 1 { f32::from_sample(frame[1]) } else { l };
                    // A full ring means the engine isn't reading; dropping is the right thing.
                    if tx.slots() >= 2 {
                        let _ = tx.push(l);
                        let _ = tx.push(r);
                    }
                }
            },
            |err| eprintln!("audio input error: {err}"),
            None,
        )
        .map_err(|e| e.to_string())
}

pub fn open_input(index: usize, engine_rate: u32) -> Result<InputHandle, String> {
    let device = cpal::default_host()
        .input_devices()
        .map_err(|e| e.to_string())?
        .nth(index)
        .ok_or("input device is no longer available")?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let ratio = config.sample_rate as f32 / engine_rate as f32;
    // One second of headroom between the two device clocks.
    let (tx, rx) = RingBuffer::new(config.sample_rate as usize * 2);
    use cpal::SampleFormat as F;
    let stream = match format {
        F::F32 => build_input::<f32>(&device, config, tx),
        F::I16 => build_input::<i16>(&device, config, tx),
        F::U16 => build_input::<u16>(&device, config, tx),
        F::I32 => build_input::<i32>(&device, config, tx),
        other => Err(format!("unsupported input sample format {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(InputHandle { stream, rx, ratio })
}
