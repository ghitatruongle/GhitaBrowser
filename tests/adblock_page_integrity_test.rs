use ghitabrowser::adblock::{AdBlockConfig, AdBlocker, ResourceType};
use ghitabrowser::compatibility_diagnostics::{build_compatibility_report, CompatibilityReport};
use ghitabrowser::css_parser::parse_css;
use ghitabrowser::text_renderer::TextRenderer;
use ghitabrowser::web_runtime::PageRuntime;

#[derive(Debug)]
struct Observation {
    text_bytes: usize,
    report: CompatibilityReport,
    script_errors: usize,
    blocked_requests: usize,
}

fn render_fixture(filtering: bool) -> Observation {
    let html = std::fs::read_to_string("tests/fixtures/adblock/page-integrity.html").unwrap();
    let mut blocker = AdBlocker::new(AdBlockConfig {
        enabled: filtering,
        ..AdBlockConfig::default()
    });
    let cosmetic_css = blocker
        .cosmetic_selectors("https://article.test/")
        .into_iter()
        .map(|selector| format!("{selector} {{ display: none; }}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut page = PageRuntime::from_html(
        &html,
        parse_css(&cosmetic_css),
        1_200,
        "https://article.test/",
    )
    .unwrap();
    page.run_document().unwrap();
    let runtime = page.report_snapshot();
    let render = page.refresh_render().clone();
    let text = render.layout.as_ref().map_or_else(String::new, |layout| {
        TextRenderer::new(1_200, 800).render_to_text(layout)
    });
    let report = build_compatibility_report(
        "https://article.test/",
        None,
        Some(&runtime),
        render.layout.as_ref(),
        None,
        1_200.0,
        800.0,
    );
    if filtering {
        let _ = blocker.evaluate_resource(
            "https://tracker.other.test/collect",
            Some("https://article.test/"),
            ResourceType::Fetch,
        );
    }
    Observation {
        text_bytes: text.trim().len(),
        script_errors: runtime.errors.len(),
        blocked_requests: blocker.total_blocked(),
        report,
    }
}

#[test]
fn enabling_adblock_preserves_page_integrity() {
    let off = render_fixture(false);
    let on = render_fixture(true);
    let text_loss_percent = if off.text_bytes == 0 {
        100.0
    } else {
        off.text_bytes.saturating_sub(on.text_bytes) as f64 / off.text_bytes as f64 * 100.0
    };
    let blank_ratio_delta =
        (on.report.layout.blank_content_ratio - off.report.layout.blank_content_ratio).max(0.0);
    let new_script_errors = on.script_errors.saturating_sub(off.script_errors);

    assert!(
        text_loss_percent <= 5.0,
        "text loss: {text_loss_percent:.2}%"
    );
    assert!(
        blank_ratio_delta <= 0.02,
        "blank delta: {blank_ratio_delta:.4}"
    );
    assert_eq!(new_script_errors, 0);
    assert!(on.blocked_requests >= 1);
}
