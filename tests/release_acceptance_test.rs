use ghitabrowser::document::prepare_document;
use ghitabrowser::parser;
use ghitabrowser::Browser;

#[test]
fn five_hundred_navigations_remain_bounded() {
    let mut browser = Browser::new_in_memory();
    for index in 0..500 {
        let url = format!("https://example.test/page/{index}");
        let html = format!(
            "<html><head><title>Page {index}</title></head><body><p>{index}</p></body></html>"
        );
        browser
            .load_html(&url, &html)
            .expect("offline navigation must succeed");
    }

    let active = browser.active_tab().expect("navigation creates a tab");
    assert!(
        active.history_len() <= 60,
        "session history must remain bounded"
    );
    assert_eq!(active.title, "Page 499");
}

#[test]
fn twenty_tabs_have_bounded_memory_lifecycle() {
    let mut browser = Browser::new_in_memory();
    browser
        .load_html("https://example.test/0", "<h1>Tab 0</h1>")
        .expect("first tab");
    for index in 1..20 {
        let dom = parser::parse_html(&format!(
            "<html><body><h1>Tab {index}</h1><p>{}</p></body></html>",
            "bounded content ".repeat(40)
        ));
        browser.add_tab(
            &format!("https://example.test/{index}"),
            dom,
            &format!("Tab {index}"),
        );
    }

    assert_eq!(browser.tab_count(), 20);
    let before = browser.estimate_memory();
    assert_eq!(before.tabs.len(), 20);
    assert!(before.total_bytes > 0);

    let discarded = browser
        .discard_least_important_tab()
        .expect("an inactive tab should be discardable");
    assert!(browser.is_tab_discarded(discarded));
    let after = browser.estimate_memory();
    assert!(after.total_bytes <= before.total_bytes);
}

#[test]
fn document_pipeline_handles_deep_input_without_unbounded_tree() {
    let mut html = String::new();
    for _ in 0..700 {
        html.push_str("<div>");
    }
    html.push_str("safe");
    for _ in 0..700 {
        html.push_str("</div>");
    }

    let prepared = prepare_document(&html, "deep", &[], 1024, 768);
    assert!(prepared.layout.is_some());
    let mut maximum_depth = 0_usize;
    let mut stack = vec![(&prepared.dom, 1_usize)];
    while let Some((element, depth)) = stack.pop() {
        maximum_depth = maximum_depth.max(depth);
        stack.extend(element.children.iter().map(|child| (child, depth + 1)));
    }
    assert!(maximum_depth <= parser::MAX_DOM_DEPTH);
}

#[test]
fn runtime_version_matches_package_version() {
    assert_eq!(ghitabrowser::VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(ghitabrowser::VERSION, "2.0.0");
}
