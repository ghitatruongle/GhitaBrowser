use ghitabrowser::adblock::{AdBlockConfig, AdBlocker, ResourceType};
use ghitabrowser::document::prepare_document;

#[test]
fn malformed_and_encrypted_pdfs_fail_closed() {
    assert!(ghitabrowser::pdf::render_to_html(b"not a pdf", "bad").is_err());
    let encrypted = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Encrypt 2 0 R >> endobj\n%%EOF";
    let error = ghitabrowser::pdf::render_to_html(encrypted, "encrypted")
        .expect_err("encrypted PDF must not enter the renderer");
    assert!(error.to_string().to_ascii_lowercase().contains("encrypted"));
}

#[test]
fn active_document_subset_mutates_dom_without_cross_origin_fetch() {
    let html = r#"
        <main>
          <h1 id="status">Loading</h1>
          <form><input placeholder="Search"><button>Go</button></form>
          <script>
            document.getElementById('status').textContent='Ready';
            localStorage.setItem('mode','compact');
            fetch('/same-origin');
            fetch('https://other.test/private');
          </script>
        </main>
    "#;
    let prepared = prepare_document(html, "https://example.test/app", &[], 1024, 768);
    assert_eq!(prepared.runtime.dom_mutations, 1);
    assert_eq!(
        prepared.runtime.fetch_requests,
        vec!["https://example.test/same-origin"]
    );
    assert!(prepared.rendered_text.contains("Ready"));
    assert!(prepared.accessibility.node_count >= 4);
}

#[test]
fn css_flex_grid_and_cosmetic_filtering_interoperate() {
    let html = r#"
        <style>
          .row { display:flex; gap:8px; }
          .grid { display:grid; grid-template-columns:repeat(2, 1fr); gap:4px; }
        </style>
        <div class="row"><span>A</span><span>B</span></div>
        <div class="grid"><span>C</span><span>D</span></div>
        <aside class="sponsored-content">Advertisement</aside>
    "#;
    let blocker = AdBlocker::new(AdBlockConfig::default());
    let cosmetic_css = blocker
        .cosmetic_selectors("https://example.test")
        .into_iter()
        .map(|selector| format!("{selector} {{ display:none; }}"))
        .collect::<Vec<_>>()
        .join("\n");
    let rules = ghitabrowser::css_parser::parse_css(&cosmetic_css);
    let prepared = prepare_document(html, "compatibility", &rules, 1024, 768);
    assert!(prepared.layout.is_some());
    assert!(!prepared.rendered_text.contains("Advertisement"));
}

#[test]
fn blocker_does_not_block_top_level_or_lookalike_hosts() {
    let mut blocker = AdBlocker::new(AdBlockConfig::default());
    assert!(!blocker.should_block_resource(
        "https://not-ads.test/docs/ads/guide.html",
        Some("not-ads.test"),
        ResourceType::Document,
    ));
    assert!(!blocker.should_block_resource(
        "https://cdn.not-ads.test/application.js",
        Some("news.test"),
        ResourceType::Script,
    ));
}

#[test]
fn performance_budget_reports_only_actual_violations() {
    let budget = ghitabrowser::performance::PerformanceBudget::default();
    let result = budget.evaluate(ghitabrowser::performance::NavigationMetrics {
        fetch_ms: 50,
        parse_ms: 10,
        style_ms: 10,
        layout_ms: 20,
        render_ms: 10,
        total_ms: 50,
        dom_nodes: 100,
        estimated_memory_bytes: 1024 * 1024,
    });
    assert!(result.passed());
}
