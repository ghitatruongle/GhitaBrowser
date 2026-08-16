use ghitabrowser::{parse_css, web_runtime::PageRuntime};

#[test]
fn mutation_observer_callbacks_keep_their_receiver_and_run_at_a_checkpoint() {
    let html = r#"<main><button id="target">change</button></main>"#;
    let mut page = PageRuntime::from_html(
        html,
        parse_css("button { color: #123456; }"),
        800,
        "https://localhost/app",
    )
    .unwrap();
    page.execute_script(
        r#"
        let observed = 0;
        let observer = new MutationObserver((records) => { observed = records.length; });
        observer.observe(document.getElementById('target'), { attributes: true });
        document.getElementById('target').setAttribute('data-state', 'ready');
        "#,
    )
    .unwrap();
    assert_eq!(page.evaluate("observed").unwrap().as_number(), Some(1.0));
}

#[test]
fn readable_streams_enforce_reader_locks_and_byte_chunk_values() {
    let mut page =
        PageRuntime::from_html("<main></main>", Vec::new(), 800, "https://localhost/app").unwrap();
    page.execute_script(
        r#"
        let stream = new ReadableStream([[65, 66]]);
        let reader = stream.getReader();
        let locked = stream.locked;
        let firstByte = 0;
        reader.read().then(result => { firstByte = result.value[0]; });
        "#,
    )
    .unwrap();
    assert_eq!(page.evaluate("locked").unwrap().as_boolean(), Some(true));
    assert_eq!(page.evaluate("firstByte").unwrap().as_number(), Some(65.0));
    assert!(page.evaluate("stream.getReader()").is_err());
}
