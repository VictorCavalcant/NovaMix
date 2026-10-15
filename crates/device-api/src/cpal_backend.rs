//! CPAL implementation of [`AudioBackend`] (WASAPI on Windows, optional ASIO).
//!
//! Every stream is normalized to interleaved `f32`. Sample formats other
//! than `f32` are converted through a buffer allocated when the stream is
//! built, never inside the callback.

use std::str::FromStr;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Data, DeviceId, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize};

use crate::*;

/// Largest callback buffer we pre-allocate conversion space for (frames ×
/// channels); larger callbacks are converted in chunks.
const CONVERT_CAPACITY: usize = 65_536;

pub fn available_hosts() -> Vec<String> {
    cpal::available_hosts().into_iter().map(|h| h.name().to_ascii_lowercase()).collect()
}

pub struct CpalBackend {
    host: cpal::Host,
}

fn map_error(e: &cpal::Error) -> DeviceError {
    use cpal::ErrorKind as K;
    let kind = match e.kind() {
        K::DeviceNotAvailable | K::StreamInvalidated | K::DeviceChanged => DeviceErrorKind::Disconnected,
        K::DeviceBusy => DeviceErrorKind::Busy,
        K::UnsupportedConfig | K::UnsupportedOperation | K::InvalidInput => DeviceErrorKind::Unsupported,
        K::PermissionDenied => DeviceErrorKind::PermissionDenied,
        K::Xrun => DeviceErrorKind::Xrun,
        _ => DeviceErrorKind::Backend,
    };
    DeviceError::new(kind, e.to_string())
}

fn form_of(t: cpal::DeviceType) -> DeviceForm {
    use cpal::DeviceType as T;
    match t {
        T::Speaker => DeviceForm::Speakers,
        T::Headphones | T::Earpiece | T::HearingAid => DeviceForm::Headphones,
        T::Headset | T::Handset => DeviceForm::Headset,
        T::Microphone => DeviceForm::Microphone,
        T::Virtual => DeviceForm::Virtual,
        _ => DeviceForm::Unknown,
    }
}

fn interface_of(i: cpal::InterfaceType) -> &'static str {
    use cpal::InterfaceType as I;
    match i {
        I::BuiltIn => "builtin",
        I::Usb => "usb",
        I::Bluetooth => "bluetooth",
        I::Pci => "pci",
        I::FireWire => "firewire",
        I::Thunderbolt => "thunderbolt",
        I::Hdmi => "hdmi",
        I::Line => "line",
        I::Spdif => "spdif",
        I::Network => "network",
        I::Virtual => "virtual",
        I::DisplayPort => "displayport",
        I::Aggregate => "aggregate",
        _ => "unknown",
    }
}

fn format_name(f: SampleFormat) -> String {
    format!("{f:?}").to_lowercase()
}

impl CpalBackend {
    pub fn new(host: Option<&str>) -> Result<Self, DeviceError> {
        let host = match host {
            None => cpal::default_host(),
            Some(name) => {
                let id = cpal::available_hosts()
                    .into_iter()
                    .find(|h| h.name().eq_ignore_ascii_case(name))
                    .ok_or_else(|| DeviceError::new(DeviceErrorKind::NotFound, format!("audio host '{name}' not available")))?;
                cpal::host_from_id(id).map_err(|e| map_error(&e))?
            }
        };
        Ok(Self { host })
    }

    fn find(&self, id: &str) -> Result<cpal::Device, DeviceError> {
        let parsed = DeviceId::from_str(id).map_err(|e| DeviceError::new(DeviceErrorKind::NotFound, e.to_string()))?;
        self.host.device_by_id(&parsed).ok_or_else(|| DeviceError::new(DeviceErrorKind::NotFound, format!("device not found: {id}")))
    }

    fn info_for(&self, dev: &cpal::Device, direction: Direction, default_id: Option<&str>) -> Option<DeviceInfo> {
        let id = dev.id().ok()?.to_string();
        let desc = dev.description().ok();
        let name = desc.as_ref().map(|d| d.name().to_string()).unwrap_or_else(|| dev.to_string());
        let default = match direction {
            Direction::Input => dev.default_input_config().ok()?,
            Direction::Output => dev.default_output_config().ok()?,
        };
        let ranges: Vec<cpal::SupportedStreamConfigRange> = match direction {
            Direction::Input => dev.supported_input_configs().map(|it| it.collect()).unwrap_or_default(),
            Direction::Output => dev.supported_output_configs().map(|it| it.collect()).unwrap_or_default(),
        };
        let mut rates: Vec<u32> =
            STANDARD_RATES.iter().copied().filter(|r| ranges.iter().any(|c| c.min_sample_rate() <= *r && *r <= c.max_sample_rate())).collect();
        if !rates.contains(&default.sample_rate()) {
            rates.push(default.sample_rate());
            rates.sort_unstable();
        }
        let buffer_range = match default.buffer_size() {
            SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            SupportedBufferSize::Unknown => None,
        };
        let form = desc.as_ref().map(|d| form_of(d.device_type())).unwrap_or_default();
        let interface = desc.as_ref().map(|d| interface_of(d.interface_type())).unwrap_or("unknown");
        let is_virtual = interface == "virtual" || form == DeviceForm::Virtual;
        Some(DeviceInfo {
            is_default: default_id == Some(id.as_str()),
            id,
            name,
            direction,
            channels: default.channels(),
            default_sample_rate: default.sample_rate(),
            sample_rates: rates,
            buffer_range,
            sample_format: format_name(default.sample_format()),
            form,
            interface: interface.to_string(),
            is_virtual,
            manufacturer: desc.as_ref().and_then(|d| d.manufacturer().map(String::from)),
        })
    }

    fn buffer_size(req: &StreamRequest, range: &SupportedBufferSize) -> BufferSize {
        match (req.buffer_frames, range) {
            (Some(b), SupportedBufferSize::Range { min, max }) => BufferSize::Fixed(b.clamp(*min, *max)),
            (Some(b), SupportedBufferSize::Unknown) => BufferSize::Fixed(b),
            (None, _) => BufferSize::Default,
        }
    }
}

struct CpalStream {
    _stream: cpal::Stream,
    info: StreamInfo,
}

impl StreamHandle for CpalStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}

fn convert_in<T: SizedSample>(src: &[T], dst: &mut [f32])
where
    f32: FromSample<T>,
{
    for (d, s) in dst.iter_mut().zip(src) {
        *d = s.to_sample::<f32>();
    }
}

fn convert_out<T: SizedSample + FromSample<f32>>(src: &[f32], dst: &mut [T]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = T::from_sample(*s);
    }
}

/// Converts any supported input format to f32 and calls `cb` in chunks.
fn dispatch_input(data: &Data, scratch: &mut [f32], channels: usize, cb: &mut InputCallback) {
    macro_rules! conv {
        ($t:ty) => {{
            if let Some(src) = data.as_slice::<$t>() {
                let chunk = (scratch.len() / channels).max(1) * channels;
                for part in src.chunks(chunk) {
                    convert_in(part, &mut scratch[..part.len()]);
                    cb(&scratch[..part.len()], channels);
                }
            }
        }};
    }
    match data.sample_format() {
        SampleFormat::F32 => {
            if let Some(src) = data.as_slice::<f32>() {
                cb(src, channels);
            }
        }
        SampleFormat::I16 => conv!(i16),
        SampleFormat::I32 => conv!(i32),
        SampleFormat::U8 => conv!(u8),
        SampleFormat::U16 => conv!(u16),
        SampleFormat::F64 => conv!(f64),
        SampleFormat::I24 => conv!(cpal::I24),
        SampleFormat::I8 => conv!(i8),
        _ => {}
    }
}

fn dispatch_output(data: &mut Data, scratch: &mut [f32], channels: usize, cb: &mut OutputCallback) {
    macro_rules! conv {
        ($t:ty) => {{
            if let Some(dst) = data.as_slice_mut::<$t>() {
                let chunk = (scratch.len() / channels).max(1) * channels;
                for part in dst.chunks_mut(chunk) {
                    let s = &mut scratch[..part.len()];
                    cb(s, channels);
                    convert_out(s, part);
                }
            }
        }};
    }
    match data.sample_format() {
        SampleFormat::F32 => {
            if let Some(dst) = data.as_slice_mut::<f32>() {
                cb(dst, channels);
            }
        }
        SampleFormat::I16 => conv!(i16),
        SampleFormat::I32 => conv!(i32),
        SampleFormat::U8 => conv!(u8),
        SampleFormat::U16 => conv!(u16),
        SampleFormat::F64 => conv!(f64),
        SampleFormat::I24 => conv!(cpal::I24),
        SampleFormat::I8 => conv!(i8),
        _ => {}
    }
}

impl AudioBackend for CpalBackend {
    fn host_name(&self) -> String {
        self.host.id().name().to_string()
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let default_in = self.host.default_input_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
        let default_out = self.host.default_output_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
        let mut out = Vec::new();
        for dev in self.host.devices().map_err(|e| map_error(&e))? {
            if dev.supports_input()
                && let Some(i) = self.info_for(&dev, Direction::Input, default_in.as_deref())
            {
                out.push(i);
            }
            if dev.supports_output()
                && let Some(i) = self.info_for(&dev, Direction::Output, default_out.as_deref())
            {
                out.push(i);
            }
        }
        Ok(out)
    }

    fn open_input(&self, req: &StreamRequest, mut data: InputCallback, mut error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        let dev = self.find(&req.device_id)?;
        // Loopback capture uses the output device's mix format.
        let supported = if req.loopback { dev.default_output_config() } else { dev.default_input_config() }.map_err(|e| map_error(&e))?;
        let config = StreamConfig {
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            buffer_size: Self::buffer_size(req, supported.buffer_size()),
        };
        let format = supported.sample_format();
        let channels = config.channels as usize;
        let mut scratch = vec![0.0f32; CONVERT_CAPACITY];
        let stream = dev
            .build_input_stream_raw(
                config,
                format,
                move |d: &Data, _: &cpal::InputCallbackInfo| dispatch_input(d, &mut scratch, channels, &mut data),
                move |e| error(map_error(&e)),
                Some(std::time::Duration::from_secs(5)),
            )
            .map_err(|e| map_error(&e))?;
        stream.play().map_err(|e| map_error(&e))?;
        let name = dev.description().map(|d| d.name().to_string()).unwrap_or_else(|_| dev.to_string());
        let info = StreamInfo {
            device_id: req.device_id.clone(),
            device_name: name,
            direction: Direction::Input,
            loopback: req.loopback,
            sample_rate: config.sample_rate,
            channels: config.channels,
            buffer_frames: stream.buffer_size().ok(),
            sample_format: format_name(format),
        };
        Ok(Box::new(CpalStream { _stream: stream, info }))
    }

    fn open_output(&self, req: &StreamRequest, data: OutputCallback, error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        let dev = self.find(&req.device_id)?;
        let supported = dev.default_output_config().map_err(|e| map_error(&e))?;
        // Prefer f32 at the requested rate; WASAPI output streams accept
        // other rates through the OS resampler.
        let rate = req.sample_rate.unwrap_or(supported.sample_rate());
        let has_f32 = dev
            .supported_output_configs()
            .map(|mut it| it.any(|c| c.sample_format() == SampleFormat::F32 && c.channels() == supported.channels()))
            .unwrap_or(false);
        let format = if has_f32 { SampleFormat::F32 } else { supported.sample_format() };
        let config = StreamConfig { channels: supported.channels(), sample_rate: rate, buffer_size: Self::buffer_size(req, supported.buffer_size()) };
        let channels = config.channels as usize;
        let mut scratch = vec![0.0f32; CONVERT_CAPACITY];
        let (mut data, mut error) = (data, error);
        let stream = dev
            .build_output_stream_raw(
                config,
                format,
                move |d: &mut Data, _: &cpal::OutputCallbackInfo| dispatch_output(d, &mut scratch, channels, &mut data),
                move |e| error(map_error(&e)),
                Some(std::time::Duration::from_secs(5)),
            )
            .map_err(|e| map_error(&e))?;
        stream.play().map_err(|e| map_error(&e))?;
        let name = dev.description().map(|d| d.name().to_string()).unwrap_or_else(|_| dev.to_string());
        let info = StreamInfo {
            device_id: req.device_id.clone(),
            device_name: name,
            direction: Direction::Output,
            loopback: false,
            sample_rate: config.sample_rate,
            channels: config.channels,
            buffer_frames: stream.buffer_size().ok(),
            sample_format: format_name(format),
        };
        Ok(Box::new(CpalStream { _stream: stream, info }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_list_is_not_empty() {
        assert!(!available_hosts().is_empty());
    }

    #[test]
    fn conversions_are_correct() {
        let mut out = [0.0f32; 3];
        convert_in(&[i16::MIN, 0, i16::MAX], &mut out);
        assert!((out[0] + 1.0).abs() < 1e-4 && out[1] == 0.0 && (out[2] - 1.0).abs() < 1e-4);
        let mut back = [0i16; 3];
        convert_out(&[-1.0, 0.0, 1.0], &mut back);
        assert_eq!(back[1], 0);
        assert!(back[2] > 32_000);
    }

    /// Enumerates real devices. Ignored by default because CI machines may
    /// have no audio hardware; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn enumerate_real_devices() {
        let b = CpalBackend::new(None).unwrap();
        let devs = b.devices().unwrap();
        for d in &devs {
            println!("{:?} {} [{}] {} Hz {} ch {:?} {}", d.direction, d.name, d.id, d.default_sample_rate, d.channels, d.form, d.interface);
        }
    }
}
