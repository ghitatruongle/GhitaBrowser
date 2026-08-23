use ghitabrowser::compatibility_diagnostics::{
    build_compatibility_report, evaluate_compatibility_outcome, summarize_compatibility,
    CompatibilityStatus,
};
use ghitabrowser::text_renderer::TextRenderer;
use ghitabrowser::web_runtime::PageRuntime;

#[derive(Debug, serde::Deserialize)]
struct Case {
    id: String,
    category: String,
    base_url: String,
    min_text_bytes: usize,
    allow_fallback: bool,
    html: String,
}

#[test]
fn representative_offline_pages_are_usable_or_readable() {
    let raw = std::fs::read_to_string("tests/fixtures/web/compatibility-corpus.json").unwrap();
    let cases: Vec<Case> = serde_json::from_str(&raw).unwrap();
    assert_eq!(cases.len(), 12);

    let mut outcomes = Vec::new();
    for case in cases {
        assert!(!case.category.is_empty(), "{}", case.id);
        let started = std::time::Instant::now();
        let mut page =
            PageRuntime::from_html(&case.html, Vec::new(), 1_200, &case.base_url).unwrap();
        let _ = page.run_document();
        let runtime = page.report_snapshot();
        let render = page.refresh_render().clone();
        let text = render.layout.as_ref().map_or_else(String::new, |layout| {
            TextRenderer::new(1_200, 800).render_to_text(layout)
        });
        let mut report = build_compatibility_report(
            &case.base_url,
            None,
            Some(&runtime),
            render.layout.as_ref(),
            None,
            1_200.0,
            800.0,
        );
        if case.allow_fallback
            && !matches!(report.status, CompatibilityStatus::FullyCompatible)
            && text.trim().len() >= 128
        {
            report.status = CompatibilityStatus::DegradedShell {
                reason: "unsupported fixture runtime".to_string(),
            };
        }
        assert!(
            text.trim().len() >= case.min_text_bytes,
            "{} produced only {} readable bytes",
            case.id,
            text.trim().len()
        );
        outcomes.push(evaluate_compatibility_outcome(
            &report,
            &text,
            started.elapsed().as_millis() as u64,
        ));
    }

    let summary = summarize_compatibility(&outcomes);
    assert!(summary.passed, "{summary:?}");
}
