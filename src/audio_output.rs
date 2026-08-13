//! Bounded PCM output contract and Windows WASAPI adapter.

use std::collections::VecDeque;

use crate::media_core::DecodedAudioFrame;

pub trait AudioSink {
    fn sample_rate_hz(&self) -> u32;
    fn channels(&self) -> u16;
    fn enqueue(&mut self, frame: DecodedAudioFrame) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn flush(&mut self) -> Result<(), String>;
    fn queued_samples(&self) -> usize;
}

#[derive(Debug)]
pub struct MemoryAudioSink {
    sample_rate_hz: u32,
    channels: u16,
    max_samples: usize,
    queued: VecDeque<i16>,
    paused: bool,
}

impl MemoryAudioSink {
    pub fn new(sample_rate_hz: u32, channels: u16, max_seconds: u32) -> Result<Self, String> {
        validate_format(sample_rate_hz, channels)?;
        let max_samples = usize::try_from(sample_rate_hz)
            .ok()
            .and_then(|rate| rate.checked_mul(usize::from(channels)))
            .and_then(|per_second| per_second.checked_mul(max_seconds as usize))
            .filter(|samples| *samples > 0)
            .ok_or_else(|| "Audio sink sample budget overflow".to_string())?;
        Ok(Self {
            sample_rate_hz,
            channels,
            max_samples,
            queued: VecDeque::new(),
            paused: true,
        })
    }

    pub fn consume_samples(&mut self, samples: usize) -> usize {
        let consumed = samples.min(self.queued.len());
        self.queued.drain(..consumed);
        consumed
    }
}

impl AudioSink for MemoryAudioSink {
    fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn enqueue(&mut self, frame: DecodedAudioFrame) -> Result<(), String> {
        validate_frame(&frame, self.sample_rate_hz, self.channels)?;
        if self
            .queued
            .len()
            .saturating_add(frame.interleaved_samples.len())
            > self.max_samples
        {
            return Err("Audio sink queue budget exceeded".to_string());
        }
        self.queued.extend(frame.interleaved_samples);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.paused = true;
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        self.paused = false;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.queued.clear();
        Ok(())
    }

    fn queued_samples(&self) -> usize {
        self.queued.len()
    }
}

#[cfg(target_os = "windows")]
pub struct WindowsWasapiSink {
    client: windows::Win32::Media::Audio::IAudioClient,
    render: windows::Win32::Media::Audio::IAudioRenderClient,
    sample_rate_hz: u32,
    channels: u16,
    max_samples: usize,
    pending: VecDeque<i16>,
    running: bool,
    com_initialized: bool,
}

#[cfg(target_os = "windows")]
impl std::fmt::Debug for WindowsWasapiSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowsWasapiSink")
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("channels", &self.channels)
            .field("queued_samples", &self.pending.len())
            .field("running", &self.running)
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl WindowsWasapiSink {
    pub fn open(sample_rate_hz: u32, channels: u16) -> Result<Self, String> {
        use windows::Win32::Media::Audio::{
            eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX, WAVE_FORMAT_PCM,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        validate_format(sample_rate_hz, channels)?;
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| format!("Cannot create WASAPI device enumerator: {error}"))?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|error| format!("No default WASAPI render device is available: {error}"))?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| format!("Cannot activate the WASAPI audio client: {error}"))?;
        let block_align = channels
            .checked_mul(2)
            .ok_or_else(|| "WASAPI block alignment overflow".to_string())?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: channels,
            nSamplesPerSec: sample_rate_hz,
            nAvgBytesPerSec: sample_rate_hz.saturating_mul(u32::from(block_align)),
            nBlockAlign: block_align,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                1_000_000,
                0,
                std::ptr::addr_of!(format),
                None,
            )
        }
        .map_err(|error| format!("Cannot initialize the bounded WASAPI stream: {error}"))?;
        let render: IAudioRenderClient = unsafe { client.GetService() }
            .map_err(|error| format!("Cannot acquire the WASAPI render client: {error}"))?;
        let max_samples = usize::try_from(sample_rate_hz)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::from(channels))
            .saturating_mul(2);
        Ok(Self {
            client,
            render,
            sample_rate_hz,
            channels,
            max_samples,
            pending: VecDeque::new(),
            running: false,
            com_initialized,
        })
    }

    pub fn pump(&mut self) -> Result<usize, String> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let buffer_frames = unsafe { self.client.GetBufferSize() }
            .map_err(|error| format!("Cannot query WASAPI buffer size: {error}"))?;
        let padding = unsafe { self.client.GetCurrentPadding() }
            .map_err(|error| format!("Cannot query WASAPI buffer padding: {error}"))?;
        let pending_frames = self.pending.len() / usize::from(self.channels);
        let frames = pending_frames.min(buffer_frames.saturating_sub(padding) as usize);
        if frames == 0 {
            return Ok(0);
        }
        let frames_u32 =
            u32::try_from(frames).map_err(|_| "WASAPI frame request overflow".to_string())?;
        let target = unsafe { self.render.GetBuffer(frames_u32) }
            .map_err(|error| format!("Cannot acquire a WASAPI render buffer: {error}"))?;
        if target.is_null() {
            return Err("WASAPI returned a null render buffer".to_string());
        }
        let samples = frames.saturating_mul(usize::from(self.channels));
        let target = unsafe { std::slice::from_raw_parts_mut(target.cast::<i16>(), samples) };
        for sample in target {
            *sample = self.pending.pop_front().unwrap_or_default();
        }
        unsafe { self.render.ReleaseBuffer(frames_u32, 0) }
            .map_err(|error| format!("Cannot release the WASAPI render buffer: {error}"))?;
        if !self.running {
            unsafe { self.client.Start() }
                .map_err(|error| format!("Cannot start WASAPI playback: {error}"))?;
            self.running = true;
        }
        Ok(samples)
    }
}

#[cfg(target_os = "windows")]
impl AudioSink for WindowsWasapiSink {
    fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn enqueue(&mut self, frame: DecodedAudioFrame) -> Result<(), String> {
        validate_frame(&frame, self.sample_rate_hz, self.channels)?;
        if self
            .pending
            .len()
            .saturating_add(frame.interleaved_samples.len())
            > self.max_samples
        {
            return Err("WASAPI queue budget exceeded".to_string());
        }
        self.pending.extend(frame.interleaved_samples);
        let _ = self.pump()?;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        if self.running {
            unsafe { self.client.Stop() }
                .map_err(|error| format!("Cannot pause WASAPI playback: {error}"))?;
            self.running = false;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        if !self.running && !self.pending.is_empty() {
            let _ = self.pump()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.pause()?;
        unsafe { self.client.Reset() }
            .map_err(|error| format!("Cannot reset WASAPI playback: {error}"))?;
        self.pending.clear();
        Ok(())
    }

    fn queued_samples(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsWasapiSink {
    fn drop(&mut self) {
        let _ = self.pause();
        if self.com_initialized {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

fn validate_format(sample_rate_hz: u32, channels: u16) -> Result<(), String> {
    if !(8_000..=384_000).contains(&sample_rate_hz) || !(1..=8).contains(&channels) {
        return Err("Unsupported PCM output format".to_string());
    }
    Ok(())
}

fn validate_frame(
    frame: &DecodedAudioFrame,
    sample_rate_hz: u32,
    channels: u16,
) -> Result<(), String> {
    if frame.sample_rate_hz != sample_rate_hz || frame.channels != channels {
        return Err("PCM output format changed without a sink flush".to_string());
    }
    if !frame
        .interleaved_samples
        .len()
        .is_multiple_of(usize::from(channels))
    {
        return Err("PCM output is not channel aligned".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_sink_is_bounded_and_flushes_on_teardown() {
        let mut sink = MemoryAudioSink::new(48_000, 2, 1).unwrap();
        sink.enqueue(DecodedAudioFrame {
            timestamp_us: 0,
            duration_us: 20_000,
            sample_rate_hz: 48_000,
            channels: 2,
            interleaved_samples: vec![1; 1_920],
        })
        .unwrap();
        assert_eq!(sink.queued_samples(), 1_920);
        assert_eq!(sink.consume_samples(960), 960);
        sink.flush().unwrap();
        assert_eq!(sink.queued_samples(), 0);
    }
}
