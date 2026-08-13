//! Phase 17C acceptance gate: independently specified current-player
//! operations (standard Web IDL surface, not site-specific) that the
//! recorded player session and live players depend on: media element
//! src/currentSrc/error/buffered/playsInline, canPlayType,
//! MediaSource.isTypeSupported, SourceBuffer.remove/buffered/timestampOffset.

use ghitabrowser::javascript::JsvValue;
use ghitabrowser::web_runtime::PageRuntime;

fn page(html: &str) -> PageRuntime {
    let mut page = PageRuntime::from_html(html, Vec::new(), 800, "https://media.test/")
        .expect("page runtime construction must succeed");
    page.run_document().expect("inline scripts must run");
    page
}

fn text(value: JsvValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| panic!("expected string, got {value:?}"))
        .to_string()
}

fn boolean(value: JsvValue) -> bool {
    value
        .as_boolean()
        .unwrap_or_else(|| panic!("expected boolean, got {value:?}"))
}

#[test]
fn media_element_src_and_flag_properties_round_trip() {
    let mut page = page(
        "<main><video id='v'></video>\
         <script>let v=document.getElementById('v');\
         v.src='https://media.test/clip.mp4';\
         let src=v.src;let current=v.currentSrc;\
         v.playsInline=true;v.autoplay=true;v.loop=true;\
         let err=v.error;\
         </script></main>",
    );
    assert_eq!(
        text(page.evaluate("src").unwrap()),
        "https://media.test/clip.mp4"
    );
    assert_eq!(
        text(page.evaluate("current").unwrap()),
        "https://media.test/clip.mp4"
    );
    assert!(boolean(page.evaluate("v.playsInline").unwrap()));
    assert!(boolean(page.evaluate("v.autoplay").unwrap()));
    assert!(boolean(page.evaluate("v.loop").unwrap()));
    assert_eq!(page.evaluate("err"), Ok(JsvValue::Null));
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn assigning_src_resets_playback_and_media_source_attachment() {
    let init = include_bytes!("fixtures/media/mse/video-init.mp4");
    let media = include_bytes!("fixtures/media/mse/video-1.m4s");
    let bytes = |data: &[u8]| {
        format!(
            "[{}]",
            data.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
        )
    };
    let mut page = page(&format!(
        "<main><video id='v'></video>\
         <script>let v=document.getElementById('v');\
         let source=MediaSource();let buffer=source.addSourceBuffer('video/mp4; codecs=\"avc1\"');\
         buffer.appendBuffer({});buffer.appendBuffer({});\
         source.endOfStream();\
         v.srcObject=source;v.play();\
         let attached=v.srcObject!==null;\
         v.src='https://media.test/other.mp4';\
         let detached=v.srcObject===null;\
         </script></main>",
        bytes(init),
        bytes(media),
    ));
    assert!(boolean(page.evaluate("attached").unwrap()));
    assert!(
        boolean(page.evaluate("detached").unwrap()),
        "assigning src must drop the MediaSource attachment"
    );
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn can_play_type_and_is_type_supported_match_decoder_capabilities() {
    let mut page = page(
        "<main><video id='v'></video>\
         <script>let v=document.getElementById('v');\
         let avc=v.canPlayType('video/mp4; codecs=\"avc1.42E01E\"');\
         let audio=v.canPlayType('audio/mp4; codecs=\"mp4a.40.2\"');\
         let bad=v.canPlayType('video/x-weird; codecs=\"zzz\"');\
         let supported=MediaSource.isTypeSupported('video/mp4; codecs=\"avc1\"');\
         let unsupported=MediaSource.isTypeSupported('video/webm');\
         </script></main>",
    );
    assert_eq!(text(page.evaluate("avc").unwrap()), "probably");
    assert_eq!(text(page.evaluate("audio").unwrap()), "probably");
    assert_eq!(text(page.evaluate("bad").unwrap()), "");
    assert!(boolean(page.evaluate("supported").unwrap()));
    assert!(!boolean(page.evaluate("unsupported").unwrap()));
}

#[test]
fn source_buffer_remove_buffered_and_timestamp_offset() {
    let init = include_bytes!("fixtures/media/mse/video-init.mp4");
    let media = include_bytes!("fixtures/media/mse/video-1.m4s");
    let bytes = |data: &[u8]| {
        format!(
            "[{}]",
            data.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
        )
    };
    let mut page = page(&format!(
        "<main><video id='v'></video>\
         <script>let source=MediaSource();let buffer=source.addSourceBuffer('video/mp4; codecs=\"avc1\"');\
         buffer.appendBuffer({});buffer.appendBuffer({});\
         buffer.timestampOffset=1.5;\
         let offset=buffer.timestampOffset;\
         let before=buffer.buffered.length;\
         let removed=buffer.remove(0,2);\
         let after=buffer.buffered.length;\
         </script></main>",
        bytes(init),
        bytes(media),
    ));
    assert!(
        boolean(page.evaluate("before>0").unwrap()),
        "buffered must report ranges"
    );
    assert_eq!(page.evaluate("offset").unwrap(), JsvValue::Number(1.5));
    assert!(
        boolean(page.evaluate("removed>0").unwrap()),
        "remove must drop samples"
    );
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}
