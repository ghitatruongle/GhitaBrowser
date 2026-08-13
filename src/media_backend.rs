//! Decoder-backend capability discovery.
//!
//! GhitaBrowser owns media parsing, buffering and playback state. On Windows,
//! compressed samples may be handed to a Media Foundation transform selected
//! through this adapter. Fallback codecs must be registered with explicit
//! provenance and remain disabled until their license has been approved.

use std::path::Path;

use crate::media_core::{DecodedAudioFrame, DecodedVideoFrame, MediaCodec};

const MAX_FALLBACKS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderProvider {
    BrowserPcm,
    WindowsMediaFoundation,
    AuditedFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecCapability {
    pub codec: MediaCodec,
    pub available: bool,
    pub provider: DecoderProvider,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecoderCapabilities {
    pub codecs: Vec<CodecCapability>,
    pub probe_error: Option<String>,
}

impl DecoderCapabilities {
    pub fn supports(&self, codec: &MediaCodec) -> bool {
        self.codecs
            .iter()
            .any(|entry| entry.available && &entry.codec == codec)
    }

    pub fn provider(&self, codec: &MediaCodec) -> Option<DecoderProvider> {
        self.codecs
            .iter()
            .find(|entry| entry.available && &entry.codec == codec)
            .map(|entry| entry.provider)
    }
}

pub trait DecoderBackend {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> DecoderCapabilities;
}

/// Always-available, project-authored PCM path used by clear-content gates and
/// as the uncompressed audio output contract for platform decoders.
#[derive(Debug, Default)]
pub struct BrowserPcmBackend;

impl DecoderBackend for BrowserPcmBackend {
    fn name(&self) -> &'static str {
        "ghita-pcm"
    }

    fn capabilities(&self) -> DecoderCapabilities {
        DecoderCapabilities {
            codecs: vec![CodecCapability {
                codec: MediaCodec::Pcm,
                available: true,
                provider: DecoderProvider::BrowserPcm,
            }],
            probe_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditedFallback {
    pub name: String,
    pub version: String,
    pub spdx_license: String,
    pub codecs: Vec<MediaCodec>,
    pub approved: bool,
}

#[derive(Debug, Default)]
pub struct FallbackRegistry {
    entries: Vec<AuditedFallback>,
}

impl FallbackRegistry {
    pub fn register(&mut self, fallback: AuditedFallback) -> Result<(), String> {
        if self.entries.len() >= MAX_FALLBACKS {
            return Err("Codec fallback registry budget exceeded".to_string());
        }
        if fallback.name.trim().is_empty()
            || fallback.version.trim().is_empty()
            || fallback.spdx_license.trim().is_empty()
            || fallback.codecs.is_empty()
        {
            return Err("Codec fallback provenance is incomplete".to_string());
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.name == fallback.name && entry.version == fallback.version)
        {
            return Err("Codec fallback is already registered".to_string());
        }
        self.entries.push(fallback);
        Ok(())
    }

    pub fn approve(&mut self, name: &str, version: &str) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.name == name && entry.version == version)
            .ok_or_else(|| "Codec fallback is not registered".to_string())?;
        entry.approved = true;
        Ok(())
    }

    pub fn capabilities(&self) -> DecoderCapabilities {
        let codecs = self
            .entries
            .iter()
            .filter(|entry| entry.approved)
            .flat_map(|entry| entry.codecs.iter().cloned())
            .map(|codec| CodecCapability {
                codec,
                available: true,
                provider: DecoderProvider::AuditedFallback,
            })
            .collect();
        DecoderCapabilities {
            codecs,
            probe_error: None,
        }
    }

    pub fn entries(&self) -> &[AuditedFallback] {
        &self.entries
    }
}

#[derive(Debug, Default)]
pub struct WindowsMediaFoundationBackend;

impl DecoderBackend for WindowsMediaFoundationBackend {
    fn name(&self) -> &'static str {
        "windows-media-foundation"
    }

    fn capabilities(&self) -> DecoderCapabilities {
        platform_capabilities()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodedMediaAsset {
    pub video_frames: Vec<DecodedVideoFrame>,
    pub audio_frames: Vec<DecodedAudioFrame>,
}

/// Decodes a bounded local clear-content asset through the Windows platform
/// codec stack. This is an end-to-end backend gate; browser-owned streaming
/// demuxers still feed compressed samples through a separate adapter.
#[cfg(not(target_os = "windows"))]
pub fn decode_clear_content_file(_path: &Path) -> Result<DecodedMediaAsset, String> {
    Err("Windows Media Foundation is unavailable on this platform".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn decode_clear_content_bytes(_bytes: &[u8]) -> Result<DecodedMediaAsset, String> {
    Err("Windows Media Foundation is unavailable on this platform".to_string())
}

#[cfg(target_os = "windows")]
pub fn decode_clear_content_file(path: &Path) -> Result<DecodedMediaAsset, String> {
    decode_media_foundation_source(MediaFoundationSource::Path(path))
}

#[cfg(target_os = "windows")]
pub fn decode_clear_content_bytes(bytes: &[u8]) -> Result<DecodedMediaAsset, String> {
    decode_media_foundation_source(MediaFoundationSource::Memory(bytes))
}

#[cfg(target_os = "windows")]
enum MediaFoundationSource<'a> {
    Path(&'a Path),
    Memory(&'a [u8]),
}

#[cfg(target_os = "windows")]
fn decode_media_foundation_source(
    source: MediaFoundationSource<'_>,
) -> Result<DecodedMediaAsset, String> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows::core::PCWSTR;
    use windows::Win32::Media::MediaFoundation::{
        IMFMediaBuffer, IMFSourceReader, MFAudioFormat_PCM, MFCreateAttributes,
        MFCreateMFByteStreamOnStream, MFCreateMediaType, MFCreateSourceReaderFromByteStream,
        MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video, MFShutdown, MFStartup,
        MFVideoFormat_RGB32, MFSTARTUP_FULL, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS,
        MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
        MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
        MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
        MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
    };
    use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED, STREAM_SEEK_SET,
    };

    const MAX_VIDEO_FRAMES: usize = 1_200;
    const MAX_AUDIO_FRAMES: usize = 16_384;
    const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;
    const MAX_VIDEO_DECODED_BYTES: usize = MAX_DECODED_BYTES * 3 / 4;
    const MAX_AUDIO_DECODED_BYTES: usize = MAX_DECODED_BYTES - MAX_VIDEO_DECODED_BYTES;
    const MAX_DIMENSION: u32 = 4_096;
    const MAX_RETAINED_VIDEO_WIDTH: u32 = 256;
    const MAX_RETAINED_VIDEO_HEIGHT: u32 = 144;
    const RETAINED_VIDEO_INTERVAL_US: i64 = 250_000;

    struct RuntimeGuard {
        com_initialized: bool,
        media_started: bool,
    }

    impl Drop for RuntimeGuard {
        fn drop(&mut self) {
            unsafe {
                if self.media_started {
                    let _ = MFShutdown();
                }
                if self.com_initialized {
                    CoUninitialize();
                }
            }
        }
    }

    fn copy_buffer(buffer: &IMFMediaBuffer, max_bytes: usize) -> Result<Vec<u8>, String> {
        let mut pointer = ptr::null_mut();
        let mut length = 0u32;
        unsafe {
            buffer
                .Lock(&mut pointer, None, Some(&mut length))
                .map_err(|error| format!("Media Foundation buffer lock failed: {error}"))?;
        }
        let result = if pointer.is_null() || length as usize > max_bytes {
            Err("Decoded Media Foundation buffer exceeds its budget".to_string())
        } else {
            Ok(unsafe { std::slice::from_raw_parts(pointer, length as usize) }.to_vec())
        };
        unsafe {
            buffer
                .Unlock()
                .map_err(|error| format!("Media Foundation buffer unlock failed: {error}"))?;
        }
        result
    }

    fn read_sample(
        reader: &IMFSourceReader,
        stream: u32,
    ) -> Result<
        (
            u32,
            i64,
            Option<windows::Win32::Media::MediaFoundation::IMFSample>,
        ),
        String,
    > {
        let mut flags = 0u32;
        let mut timestamp = 0i64;
        let mut sample = None;
        unsafe {
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|error| format!("Media Foundation sample read failed: {error}"))?;
        }
        Ok((flags, timestamp, sample))
    }

    let (wide, memory) = match source {
        MediaFoundationSource::Path(path) => {
            if !path.is_file() {
                return Err("Clear-content media fixture does not exist".to_string());
            }
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("Cannot resolve clear-content fixture: {error}"))?;
            let canonical_text = canonical.to_string_lossy();
            let source_path = canonical_text
                .strip_prefix(r"\\?\")
                .unwrap_or(canonical_text.as_ref())
                .to_string();
            let wide = std::ffi::OsStr::new(&source_path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            (Some(wide), None)
        }
        MediaFoundationSource::Memory(bytes) => {
            if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
                return Err("In-memory media source exceeds the 64 MB input budget".to_string());
            }
            (None, Some(bytes))
        }
    };

    let mut guard = RuntimeGuard {
        com_initialized: false,
        media_started: false,
    };
    let com_result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com_result.is_ok() {
        guard.com_initialized = true;
    }
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
        .map_err(|error| format!("Media Foundation startup failed: {error}"))?;
    guard.media_started = true;

    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }
        .map_err(|error| format!("Cannot create source reader attributes: {error}"))?;
    let attributes = attributes
        .ok_or_else(|| "Media Foundation returned no source reader attributes".to_string())?;
    unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1) }
        .map_err(|error| format!("Cannot enable source reader video processing: {error}"))?;
    let reader = if let Some(wide) = wide.as_ref() {
        unsafe { MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attributes) }
            .map_err(|error| format!("Media Foundation source reader failed: {error}"))?
    } else {
        let bytes = memory.expect("validated in-memory source");
        let stream = unsafe {
            CreateStreamOnHGlobal(
                windows::Win32::Foundation::HGLOBAL::default(),
                windows::Win32::Foundation::BOOL(1),
            )
        }
        .map_err(|error| format!("Cannot create in-memory COM stream: {error}"))?;
        let byte_count = u32::try_from(bytes.len())
            .map_err(|_| "In-memory media source length overflow".to_string())?;
        let mut written = 0u32;
        unsafe {
            stream
                .Write(bytes.as_ptr().cast(), byte_count, Some(&mut written))
                .ok()
                .map_err(|error| format!("Cannot write in-memory media stream: {error}"))?;
            stream
                .Seek(0, STREAM_SEEK_SET, None)
                .map_err(|error| format!("Cannot rewind in-memory media stream: {error}"))?;
        }
        if written != byte_count {
            return Err("In-memory media stream write was truncated".to_string());
        }
        let byte_stream = unsafe { MFCreateMFByteStreamOnStream(&stream) }
            .map_err(|error| format!("Cannot wrap in-memory media stream: {error}"))?;
        unsafe { MFCreateSourceReaderFromByteStream(&byte_stream, &attributes) }
            .map_err(|error| format!("Media Foundation byte-stream reader failed: {error}"))?
    };
    unsafe {
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(|error| format!("Media Foundation stream reset failed: {error}"))?;
    }

    let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let video_type = unsafe { MFCreateMediaType() }
        .map_err(|error| format!("Cannot create video output type: {error}"))?;
    let video_configured = unsafe {
        video_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .and_then(|_| video_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))
            .and_then(|_| reader.SetStreamSelection(video_stream, true))
            .and_then(|_| reader.SetCurrentMediaType(video_stream, None, &video_type))
            .is_ok()
    };
    let audio_stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    let audio_type = unsafe { MFCreateMediaType() }
        .map_err(|error| format!("Cannot create audio output type: {error}"))?;
    let audio_configured = unsafe {
        audio_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .and_then(|_| audio_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM))
            .and_then(|_| reader.SetStreamSelection(audio_stream, true))
            .and_then(|_| reader.SetCurrentMediaType(audio_stream, None, &audio_type))
            .is_ok()
    };
    if !video_configured && !audio_configured {
        return Err("Media Foundation found no supported audio or video stream".to_string());
    }
    let audio_format = if audio_configured {
        let current_audio = unsafe { reader.GetCurrentMediaType(audio_stream) }
            .map_err(|error| format!("Cannot inspect audio output type: {error}"))?;
        let channels = unsafe { current_audio.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }
            .map_err(|error| format!("Audio output has no channel count: {error}"))?;
        let sample_rate = unsafe { current_audio.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }
            .map_err(|error| format!("Audio output has no sample rate: {error}"))?;
        let bits = unsafe { current_audio.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE) }.unwrap_or(16);
        if !(1..=8).contains(&channels) || !(8_000..=384_000).contains(&sample_rate) || bits != 16 {
            return Err("Media Foundation negotiated unsupported PCM output".to_string());
        }
        Some((channels, sample_rate))
    } else {
        None
    };
    let video_format = if video_configured {
        let current_video = unsafe { reader.GetCurrentMediaType(video_stream) }
            .map_err(|error| format!("Cannot inspect video output type: {error}"))?;
        let packed_size = unsafe { current_video.GetUINT64(&MF_MT_FRAME_SIZE) }
            .map_err(|error| format!("Video output has no frame size: {error}"))?;
        let width = (packed_size >> 32) as u32;
        let height = packed_size as u32;
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err("Media Foundation video dimensions exceed the browser budget".to_string());
        }
        let stride = unsafe { current_video.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|value| (value as i32).unsigned_abs() as usize)
            .unwrap_or(width as usize * 4);
        let row_bytes = width as usize * 4;
        if stride < row_bytes {
            return Err("Media Foundation video stride is invalid".to_string());
        }
        Some((width, height, stride))
    } else {
        None
    };

    let mut output = DecodedMediaAsset::default();
    let mut decoded_video_bytes = 0usize;
    let mut decoded_audio_bytes = 0usize;
    if let Some((width, height, stride)) = video_format {
        let scale = (MAX_RETAINED_VIDEO_WIDTH as f64 / width as f64)
            .min(MAX_RETAINED_VIDEO_HEIGHT as f64 / height as f64)
            .min(1.0);
        let retained_width = ((width as f64 * scale).round() as u32).max(1);
        let retained_height = ((height as f64 * scale).round() as u32).max(1);
        let retained_row_bytes = retained_width as usize * 4;
        let retained_rgba_length = retained_row_bytes
            .checked_mul(retained_height as usize)
            .ok_or_else(|| "Retained video size overflow".to_string())?;
        let mut last_retained_timestamp_us = None;
        loop {
            if output.video_frames.len() >= MAX_VIDEO_FRAMES {
                break;
            }
            let (flags, timestamp_100ns, sample) = read_sample(&reader, video_stream)?;
            if let Some(sample) = sample {
                let duration_100ns = unsafe { sample.GetSampleDuration() }.unwrap_or_default();
                let timestamp_us = timestamp_100ns / 10;
                let retain = last_retained_timestamp_us.is_none_or(|last| {
                    timestamp_us.saturating_sub(last) >= RETAINED_VIDEO_INTERVAL_US
                });
                if !retain {
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        break;
                    }
                    continue;
                }
                let contiguous = unsafe { sample.ConvertToContiguousBuffer() }
                    .map_err(|error| format!("Cannot join video sample buffers: {error}"))?;
                let required = stride
                    .checked_mul(height as usize)
                    .ok_or_else(|| "Decoded video size overflow".to_string())?;
                if decoded_video_bytes.saturating_add(retained_rgba_length)
                    > MAX_VIDEO_DECODED_BYTES
                {
                    break;
                }
                let bytes = copy_buffer(&contiguous, MAX_DECODED_BYTES)?;
                if bytes.len() < required {
                    return Err("Decoded RGB32 sample is truncated".to_string());
                }
                let mut rgba = vec![0u8; retained_rgba_length];
                for target_y in 0..retained_height as usize {
                    let source_y = target_y * height as usize / retained_height as usize;
                    for target_x in 0..retained_width as usize {
                        let source_x = target_x * width as usize / retained_width as usize;
                        let source_offset = source_y * stride + source_x * 4;
                        let target_offset = target_y * retained_row_bytes + target_x * 4;
                        let bgra = &bytes[source_offset..source_offset + 4];
                        rgba[target_offset..target_offset + 4]
                            .copy_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
                    }
                }
                decoded_video_bytes = decoded_video_bytes
                    .checked_add(rgba.len())
                    .ok_or_else(|| "Decoded media byte count overflow".to_string())?;
                if decoded_video_bytes > MAX_VIDEO_DECODED_BYTES {
                    return Err("Decoded media byte budget exceeded".to_string());
                }
                output.video_frames.push(DecodedVideoFrame {
                    timestamp_us,
                    duration_us: u64::try_from(duration_100ns.max(0) / 10).unwrap_or_default(),
                    width: retained_width,
                    height: retained_height,
                    rgba,
                });
                last_retained_timestamp_us = Some(timestamp_us);
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
    }

    if let Some((channels, sample_rate)) = audio_format {
        loop {
            if output.audio_frames.len() >= MAX_AUDIO_FRAMES {
                break;
            }
            let (flags, timestamp_100ns, sample) = read_sample(&reader, audio_stream)?;
            if let Some(sample) = sample {
                let duration_100ns = unsafe { sample.GetSampleDuration() }.unwrap_or_default();
                let contiguous = unsafe { sample.ConvertToContiguousBuffer() }
                    .map_err(|error| format!("Cannot join audio sample buffers: {error}"))?;
                let bytes = copy_buffer(&contiguous, MAX_DECODED_BYTES)?;
                if !bytes.len().is_multiple_of(2)
                    || !(bytes.len() / 2).is_multiple_of(channels as usize)
                {
                    return Err("Decoded PCM sample is misaligned".to_string());
                }
                if decoded_audio_bytes.saturating_add(bytes.len()) > MAX_AUDIO_DECODED_BYTES {
                    break;
                }
                decoded_audio_bytes = decoded_audio_bytes
                    .checked_add(bytes.len())
                    .ok_or_else(|| "Decoded media byte count overflow".to_string())?;
                if decoded_audio_bytes > MAX_AUDIO_DECODED_BYTES {
                    return Err("Decoded media byte budget exceeded".to_string());
                }
                output.audio_frames.push(DecodedAudioFrame {
                    timestamp_us: timestamp_100ns / 10,
                    duration_us: u64::try_from(duration_100ns.max(0) / 10).unwrap_or_default(),
                    sample_rate_hz: sample_rate,
                    channels: channels as u16,
                    interleaved_samples: bytes
                        .chunks_exact(2)
                        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                        .collect(),
                });
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
    }

    if output.video_frames.is_empty() && output.audio_frames.is_empty() {
        return Err(format!(
            "Clear-content media produced no output (video frames: {}, audio frames: {})",
            output.video_frames.len(),
            output.audio_frames.len()
        ));
    }
    Ok(output)
}

#[cfg(not(target_os = "windows"))]
fn platform_capabilities() -> DecoderCapabilities {
    DecoderCapabilities {
        codecs: Vec::new(),
        probe_error: Some("Windows Media Foundation is unavailable on this platform".to_string()),
    }
}

#[cfg(target_os = "windows")]
fn platform_capabilities() -> DecoderCapabilities {
    use std::ffi::c_void;
    use std::ptr;

    use windows::core::GUID;
    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, MFAudioFormat_AAC, MFAudioFormat_Opus, MFAudioFormat_Vorbis,
        MFMediaType_Audio, MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx, MFVideoFormat_AV1,
        MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_VP80, MFVideoFormat_VP90,
        MFSTARTUP_FULL, MFT_CATEGORY_AUDIO_DECODER, MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG,
        MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_LOCALMFT,
        MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_REGISTER_TYPE_INFO, MF_VERSION,
    };
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
    };

    struct MediaFoundationGuard {
        com_initialized: bool,
        media_started: bool,
    }

    impl Drop for MediaFoundationGuard {
        fn drop(&mut self) {
            unsafe {
                if self.media_started {
                    let _ = MFShutdown();
                }
                if self.com_initialized {
                    CoUninitialize();
                }
            }
        }
    }

    unsafe fn decoder_available(category: GUID, major: GUID, subtype: GUID) -> bool {
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: major,
            guidSubtype: subtype,
        };
        let combined_flags = MFT_ENUM_FLAG(
            MFT_ENUM_FLAG_SYNCMFT.0
                | MFT_ENUM_FLAG_ASYNCMFT.0
                | MFT_ENUM_FLAG_HARDWARE.0
                | MFT_ENUM_FLAG_LOCALMFT.0
                | MFT_ENUM_FLAG_SORTANDFILTER.0,
        );
        let mut activations: *mut Option<IMFActivate> = ptr::null_mut();
        let mut count = 0u32;
        if MFTEnumEx(
            category,
            combined_flags,
            Some(&input),
            None,
            &mut activations,
            &mut count,
        )
        .is_err()
        {
            return false;
        }
        if !activations.is_null() {
            let entries = std::slice::from_raw_parts_mut(activations, count as usize);
            for entry in entries {
                let _ = entry.take();
            }
            CoTaskMemFree(Some(activations.cast::<c_void>()));
        }
        count > 0
    }

    let mut guard = MediaFoundationGuard {
        com_initialized: false,
        media_started: false,
    };
    let com_result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com_result.is_ok() {
        guard.com_initialized = true;
    }
    if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
        return DecoderCapabilities {
            codecs: Vec::new(),
            probe_error: Some(format!("Media Foundation startup failed: {error}")),
        };
    }
    guard.media_started = true;

    let formats = [
        (
            MediaCodec::Avc,
            MFT_CATEGORY_VIDEO_DECODER,
            MFMediaType_Video,
            MFVideoFormat_H264,
        ),
        (
            MediaCodec::Hevc,
            MFT_CATEGORY_VIDEO_DECODER,
            MFMediaType_Video,
            MFVideoFormat_HEVC,
        ),
        (
            MediaCodec::Vp8,
            MFT_CATEGORY_VIDEO_DECODER,
            MFMediaType_Video,
            MFVideoFormat_VP80,
        ),
        (
            MediaCodec::Vp9,
            MFT_CATEGORY_VIDEO_DECODER,
            MFMediaType_Video,
            MFVideoFormat_VP90,
        ),
        (
            MediaCodec::Av1,
            MFT_CATEGORY_VIDEO_DECODER,
            MFMediaType_Video,
            MFVideoFormat_AV1,
        ),
        (
            MediaCodec::Aac,
            MFT_CATEGORY_AUDIO_DECODER,
            MFMediaType_Audio,
            MFAudioFormat_AAC,
        ),
        (
            MediaCodec::Opus,
            MFT_CATEGORY_AUDIO_DECODER,
            MFMediaType_Audio,
            MFAudioFormat_Opus,
        ),
        (
            MediaCodec::Vorbis,
            MFT_CATEGORY_AUDIO_DECODER,
            MFMediaType_Audio,
            MFAudioFormat_Vorbis,
        ),
    ];
    let codecs = formats
        .into_iter()
        .map(|(codec, category, major, subtype)| CodecCapability {
            codec,
            available: unsafe { decoder_available(category, major, subtype) },
            provider: DecoderProvider::WindowsMediaFoundation,
        })
        .collect();
    DecoderCapabilities {
        codecs,
        probe_error: com_result.err().map(|error| {
            format!("COM apartment was already initialized with another model: {error}")
        }),
    }
}

pub fn merged_capabilities(
    platform: &dyn DecoderBackend,
    fallbacks: &FallbackRegistry,
) -> DecoderCapabilities {
    let mut merged = BrowserPcmBackend.capabilities();
    let platform = platform.capabilities();
    merged.codecs.extend(platform.codecs);
    merged.codecs.extend(fallbacks.capabilities().codecs);
    merged.probe_error = platform.probe_error;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_requires_complete_provenance_and_explicit_approval() {
        let mut registry = FallbackRegistry::default();
        registry
            .register(AuditedFallback {
                name: "example-opus".to_string(),
                version: "1.0.0".to_string(),
                spdx_license: "BSD-3-Clause".to_string(),
                codecs: vec![MediaCodec::Opus],
                approved: false,
            })
            .unwrap();
        assert!(!registry.capabilities().supports(&MediaCodec::Opus));
        registry.approve("example-opus", "1.0.0").unwrap();
        assert!(registry.capabilities().supports(&MediaCodec::Opus));
    }

    #[test]
    fn pcm_is_always_available_without_a_platform_codec() {
        let capabilities = BrowserPcmBackend.capabilities();
        assert_eq!(
            capabilities.provider(&MediaCodec::Pcm),
            Some(DecoderProvider::BrowserPcm)
        );
    }
}
