use ghitabrowser::html_media::HtmlMediaElement;
use ghitabrowser::media_backend::{
    decode_clear_content_bytes, decode_clear_content_file, merged_capabilities, CodecCapability,
    DecoderBackend, DecoderCapabilities, DecoderProvider, FallbackRegistry,
    WindowsMediaFoundationBackend,
};
use ghitabrowser::media_core::{
    DecodedFrame, MediaCodec, MediaDecoder, MediaDemuxer, Pcm16Decoder, WavePcmDemuxer,
};
use ghitabrowser::mse::MediaSource;
use ghitabrowser::youtube::{RecordedPlaybackAssets, YouTubePlayerSession};

fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(payload);
    output
}

fn full_box(version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut output = vec![
        version,
        (flags >> 16) as u8,
        (flags >> 8) as u8,
        flags as u8,
    ];
    output.extend_from_slice(body);
    output
}

fn mp4_init(track_id: u32, timescale: u32, handler: &[u8; 4], codec: &[u8; 4]) -> Vec<u8> {
    let mut tkhd_body = vec![0; 8];
    tkhd_body.extend_from_slice(&track_id.to_be_bytes());
    tkhd_body.extend_from_slice(&[0; 4]);
    let tkhd = boxed(b"tkhd", &full_box(0, 0, &tkhd_body));
    let mut mdhd_body = vec![0; 8];
    mdhd_body.extend_from_slice(&timescale.to_be_bytes());
    mdhd_body.extend_from_slice(&0u32.to_be_bytes());
    let mdhd = boxed(b"mdhd", &full_box(0, 0, &mdhd_body));
    let mut hdlr_body = vec![0; 4];
    hdlr_body.extend_from_slice(handler);
    let hdlr = boxed(b"hdlr", &full_box(0, 0, &hdlr_body));
    let sample_entry = boxed(codec, &[0; 8]);
    let mut stsd_body = 1u32.to_be_bytes().to_vec();
    stsd_body.extend_from_slice(&sample_entry);
    let stsd = boxed(b"stsd", &full_box(0, 0, &stsd_body));
    let stbl = boxed(b"stbl", &stsd);
    let minf = boxed(b"minf", &stbl);
    let mut mdia_payload = Vec::new();
    mdia_payload.extend_from_slice(&mdhd);
    mdia_payload.extend_from_slice(&hdlr);
    mdia_payload.extend_from_slice(&minf);
    let mdia = boxed(b"mdia", &mdia_payload);
    let mut trak_payload = tkhd;
    trak_payload.extend_from_slice(&mdia);
    let moov = boxed(b"moov", &boxed(b"trak", &trak_payload));
    let mut ftyp_payload = b"isom".to_vec();
    ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"isom");
    let mut output = boxed(b"ftyp", &ftyp_payload);
    output.extend_from_slice(&moov);
    output
}

fn mp4_media(track_id: u32, duration: u32, count: usize, payload: &[u8]) -> Vec<u8> {
    let mut tfhd_body = track_id.to_be_bytes().to_vec();
    tfhd_body.extend_from_slice(&duration.to_be_bytes());
    let tfhd = boxed(b"tfhd", &full_box(0, 0x000008, &tfhd_body));
    let tfdt = boxed(b"tfdt", &full_box(1, 0, &0u64.to_be_bytes()));
    let mut trun_body = (count as u32).to_be_bytes().to_vec();
    for _ in 0..count {
        trun_body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    }
    let trun = boxed(b"trun", &full_box(0, 0x000200, &trun_body));
    let mut traf_payload = tfhd;
    traf_payload.extend_from_slice(&tfdt);
    traf_payload.extend_from_slice(&trun);
    let moof = boxed(b"moof", &boxed(b"traf", &traf_payload));
    let mdat_payload = payload.repeat(count);
    let mut output = moof;
    output.extend_from_slice(&boxed(b"mdat", &mdat_payload));
    output
}

fn wave_fixture(frames: usize) -> Vec<u8> {
    let channels = 2u16;
    let sample_rate = 48_000u32;
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let mut data = Vec::new();
    for index in 0..frames {
        let value = (index as i16).wrapping_mul(17);
        data.extend_from_slice(&value.to_le_bytes());
        data.extend_from_slice(&(-value).to_le_bytes());
    }
    let mut wave = Vec::new();
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16u32.to_le_bytes());
    wave.extend_from_slice(&1u16.to_le_bytes());
    wave.extend_from_slice(&channels.to_le_bytes());
    wave.extend_from_slice(&sample_rate.to_le_bytes());
    wave.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
    wave.extend_from_slice(&block_align.to_le_bytes());
    wave.extend_from_slice(&bits.to_le_bytes());
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wave.extend_from_slice(&data);
    wave
}

fn av_capabilities() -> DecoderCapabilities {
    DecoderCapabilities {
        codecs: vec![
            CodecCapability {
                codec: MediaCodec::Avc,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Aac,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
        ],
        probe_error: None,
    }
}

fn recorded_youtube_html() -> String {
    let initial = serde_json::json!({
        "contents": {"videoRenderer": {
            "videoId": "ghitaVideo1",
            "title": {"runs": [{"text": "Ghita recorded media"}]},
            "thumbnail": {"thumbnails": [{"url": "https://img.test/gate.jpg"}]},
            "lengthText": {"simpleText": "0:08"}
        }}
    });
    let player = serde_json::json!({
        "playabilityStatus": {"status": "OK"},
        "videoDetails": {
            "videoId": "ghitaVideo1", "title": "Ghita recorded media", "lengthSeconds": "8"
        },
        "streamingData": {"adaptiveFormats": [
            {
                "itag": 137, "mimeType": "video/mp4; codecs=\"avc1.640028\"",
                "url": "https://media.test/video.mp4", "bitrate": 4000000,
                "width": 1920, "height": 1080
            },
            {
                "itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                "url": "https://media.test/audio.mp4", "bitrate": 128000,
                "audioQuality": "AUDIO_QUALITY_MEDIUM"
            }
        ]}
    });
    format!(
        "<html><script>var ytInitialData={initial};var ytInitialPlayerResponse={player};</script></html>"
    )
}

#[test]
fn phase15_clear_pcm_content_is_really_demuxed_and_decoded() {
    let wave = wave_fixture(4_800);
    let mut demuxer = WavePcmDemuxer::new();
    let encoded = demuxer.push_bytes(&wave, true).unwrap();
    let mut decoder = Pcm16Decoder::new(demuxer.format().unwrap()).unwrap();
    let decoded = encoded
        .into_iter()
        .flat_map(|sample| decoder.decode(sample).unwrap())
        .collect::<Vec<_>>();
    assert!(!decoded.is_empty());
    assert_eq!(decoded[0].sample_rate_hz, 48_000);
    assert_eq!(decoded[0].channels, 2);
    assert!(
        decoded
            .iter()
            .map(|frame| DecodedFrame::Audio(frame.clone()).estimated_bytes())
            .sum::<usize>()
            <= wave.len()
    );
}

#[cfg(target_os = "windows")]
#[test]
fn phase15_media_foundation_capabilities_are_probed_on_the_host() {
    let platform = WindowsMediaFoundationBackend;
    let capabilities = merged_capabilities(&platform, &FallbackRegistry::default());
    assert!(capabilities.supports(&MediaCodec::Pcm));
    assert!(capabilities.supports(&MediaCodec::Avc));
    assert!(capabilities.supports(&MediaCodec::Aac));
    assert_eq!(platform.name(), "windows-media-foundation");
    assert!(capabilities.codecs.len() >= 9);
    assert!(
        capabilities.probe_error.is_none(),
        "{:?}",
        capabilities.probe_error
    );
}

#[cfg(target_os = "windows")]
#[test]
fn phase15_media_foundation_decodes_real_avc_aac_frames() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/media/clear-avc-aac.mp4");
    let decoded = decode_clear_content_file(&fixture).unwrap();
    assert!(!decoded.video_frames.is_empty());
    assert!(!decoded.audio_frames.is_empty());
    assert_eq!(decoded.video_frames[0].width, 64);
    assert_eq!(decoded.video_frames[0].height, 64);
    assert_eq!(
        decoded.video_frames[0].rgba.len(),
        64usize * 64usize * 4usize
    );
    assert_eq!(decoded.audio_frames[0].sample_rate_hz, 48_000);
    assert!(decoded.audio_frames[0]
        .interleaved_samples
        .iter()
        .any(|sample| *sample != 0));
}

#[cfg(target_os = "windows")]
#[test]
fn phase17_media_foundation_decodes_a_bounded_in_memory_byte_stream() {
    let bytes = include_bytes!("fixtures/media/clear-avc-aac.mp4");
    let decoded = decode_clear_content_bytes(bytes).unwrap();
    assert!(!decoded.video_frames.is_empty());
    assert!(!decoded.audio_frames.is_empty());
    assert_eq!(decoded.video_frames[0].rgba.len(), 64 * 64 * 4);
    assert_eq!(decoded.audio_frames[0].sample_rate_hz, 48_000);
}

#[test]
fn phase16_mse_adaptive_stream_seek_and_underflow_recovery_are_interactive() {
    let capabilities = av_capabilities();
    let mut source = MediaSource::new();
    source.open().unwrap();
    let video = source
        .add_source_buffer("video/mp4; codecs=\"avc1.640028\"", &capabilities)
        .unwrap();
    let audio = source
        .add_source_buffer("audio/mp4; codecs=\"mp4a.40.2\"", &capabilities)
        .unwrap();
    source
        .append_buffer(video, &mp4_init(1, 1_000, b"vide", b"avc1"))
        .unwrap();
    source
        .append_buffer(audio, &mp4_init(2, 48_000, b"soun", b"mp4a"))
        .unwrap();
    source
        .append_buffer(video, &mp4_media(1, 1_000, 8, &[1, 2, 3]))
        .unwrap();
    source
        .append_buffer(audio, &mp4_media(2, 48_000, 8, &[4, 5]))
        .unwrap();
    source.set_duration(8_000_000).unwrap();
    source.end_of_stream().unwrap();
    let mut media = HtmlMediaElement::new();
    media.attach_media_source(source).unwrap();
    media.play().unwrap();
    media.tick(500);
    media.pause();
    media.seek(3.0).unwrap();
    media.set_playback_rate(1.5).unwrap();
    media.set_volume(0.4).unwrap();
    media.play().unwrap();
    media.tick(1_000);
    assert_eq!(media.current_time_seconds(), 4.5);
    assert!(!media.paused());
}

#[test]
fn phase16_real_fragmented_avc_aac_assets_flow_through_mse() {
    let capabilities = av_capabilities();
    let mut source = MediaSource::new();
    source.open().unwrap();
    let video = source
        .add_source_buffer("video/mp4; codecs=\"avc1.42c00a\"", &capabilities)
        .unwrap();
    let audio = source
        .add_source_buffer("audio/mp4; codecs=\"mp4a.40.2\"", &capabilities)
        .unwrap();
    source
        .append_buffer(video, include_bytes!("fixtures/media/mse/video-init.mp4"))
        .unwrap();
    source
        .append_buffer(audio, include_bytes!("fixtures/media/mse/audio-init.mp4"))
        .unwrap();
    let video_report = source
        .append_buffer(video, include_bytes!("fixtures/media/mse/video-1.m4s"))
        .unwrap();
    let audio_report = source
        .append_buffer(audio, include_bytes!("fixtures/media/mse/audio-1.m4s"))
        .unwrap();
    let video_switch = source
        .append_buffer(video, include_bytes!("fixtures/media/mse/video-2.m4s"))
        .unwrap();
    let audio_switch = source
        .append_buffer(audio, include_bytes!("fixtures/media/mse/audio-2.m4s"))
        .unwrap();
    assert!(video_report.appended_samples >= 5);
    assert!(audio_report.appended_samples >= 20);
    assert!(video_switch.appended_samples >= 1);
    assert!(audio_switch.appended_samples >= 20);
    assert!(!source.buffered().is_empty());
    assert!(source.total_queued_bytes() > 1_000);
}

#[cfg(target_os = "windows")]
#[test]
fn phase17_recorded_youtube_shell_and_player_gate_passes_without_fallback() {
    let assets = RecordedPlaybackAssets {
        video_init: mp4_init(1, 1_000, b"vide", b"avc1"),
        video_segments: vec![mp4_media(1, 1_000, 8, &[1, 2, 3])],
        audio_init: Some(mp4_init(2, 48_000, b"soun", b"mp4a")),
        audio_segments: vec![mp4_media(2, 48_000, 8, &[4, 5])],
    };
    let mut session = YouTubePlayerSession::from_recorded_page(
        "https://www.youtube.com/watch?v=ghitaVideo1",
        &recorded_youtube_html(),
        &av_capabilities(),
        assets,
    )
    .unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/media/clear-avc-aac.mp4");
    session
        .attach_decoded_output(decode_clear_content_file(&fixture).unwrap())
        .unwrap();
    session.navigate_search("ghita media").unwrap();
    session.navigate_watch("ghitaVideo1").unwrap();
    session.play().unwrap();
    session.tick(20).unwrap();
    session.pause();
    session.resume().unwrap();
    session.seek(2.0).unwrap();
    session.set_volume(0.25).unwrap();
    session.recover_after_underflow();
    assert!(session.report().passed(), "{:?}", session.report());
}
