//! Single source of truth for media byte budgets.
//!
//! Every decoded-media byte cap in the engine MUST be derived from the
//! constants here so a file that passes one stage cannot be rejected by a
//! later one (the historical defect was a 256 MB decoder feeding a 64 MB
//! page pipeline, which wasted the whole decode before failing).
//!
//! Encoded-byte queues are a DIFFERENT resource from decoded bytes: encoded
//! segments stay compact and are evictable, so their ceiling is larger and
//! documented separately instead of being silently reused.

/// Ceiling on DECODED bytes (video frames + audio samples) for one media
/// asset or page pipeline. The decoder rejects early instead of decoding
/// into memory the pipeline would immediately have to discard.
pub const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

/// Decoded-byte split between video and audio for one asset/pipeline.
pub const MAX_VIDEO_DECODED_BYTES: usize = MAX_DECODED_BYTES / 4 * 3;
pub const MAX_AUDIO_DECODED_BYTES: usize = MAX_DECODED_BYTES - MAX_VIDEO_DECODED_BYTES;

/// Ceiling on ENCODED input bytes accepted for in-memory decoding.
pub const MAX_INPUT_BYTES: usize = MAX_DECODED_BYTES;

/// Total queued ENCODED bytes across all MediaSource SourceBuffers. Encoded
/// data is an order of magnitude denser than its decoded form and is
/// evicted incrementally during playback, so this ceiling is independent of
/// [`MAX_DECODED_BYTES`].
pub const MAX_MSE_QUEUED_ENCODED_BYTES: usize = 256 * 1024 * 1024;

// Compile-time invariants: any edit that breaks the budget relationships
// fails the build itself, which is stronger than a runtime unit test.
const _: () = {
    assert!(MAX_VIDEO_DECODED_BYTES + MAX_AUDIO_DECODED_BYTES == MAX_DECODED_BYTES);
    assert!(MAX_VIDEO_DECODED_BYTES > MAX_AUDIO_DECODED_BYTES);
    assert!(MAX_INPUT_BYTES == MAX_DECODED_BYTES);
    assert!(MAX_DECODED_BYTES == 64 * 1024 * 1024);
    assert!(MAX_MSE_QUEUED_ENCODED_BYTES == 256 * 1024 * 1024);
    assert!(MAX_MSE_QUEUED_ENCODED_BYTES > MAX_DECODED_BYTES);
};
