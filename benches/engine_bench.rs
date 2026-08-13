use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ghitabrowser::{
    adblock::{AdBlockConfig, AdBlocker, ResourceType},
    css_parser,
    javascript::JsvEngine,
    layout, paint, parser, web_runtime,
};

const SAMPLE_HTML: &str = r#"
<html><head><title>Benchmark</title><style>
body { color: #202124; background: white; }
.card { margin: 8px; padding: 12px; border: 1px solid #dadce0; }
</style></head><body>
<main><h1>GhitaBrowser</h1>
<div class="card"><p>A bounded document rendering benchmark.</p>
<a href="https://example.com">Example link</a></div>
</main></body></html>
"#;

fn benchmark_document_pipeline(c: &mut Criterion) {
    c.bench_function("parse_layout_paint", |b| {
        b.iter(|| {
            let dom = parser::parse_html(black_box(SAMPLE_HTML));
            let rules = css_parser::parse_css("body { font-size: 16px; } .card { padding: 12px; }");
            let layout = layout::create_layout_tree(&dom, &rules, 1100)
                .expect("sample document must produce a layout tree");
            black_box(paint::build_display_list(&layout));
        });
    });
}

fn benchmark_javascript(c: &mut Criterion) {
    c.bench_function("javascript_bounded_loop", |b| {
        b.iter(|| {
            let mut engine = JsvEngine::new();
            black_box(
                engine
                    .eval("let i = 0; while (i < 100) { i = i + 1; } i")
                    .expect("benchmark script must evaluate"),
            );
        });
    });
}

fn benchmark_runtime_and_filtering(c: &mut Criterion) {
    c.bench_function("dom_runtime_host_bridge", |b| {
        b.iter(|| {
            let mut dom = parser::parse_html(
                "<p id='status'>old</p><script>document.getElementById('status').textContent='ready';localStorage.setItem('theme','dark');fetch('/data');</script>",
            );
            black_box(web_runtime::run_inline_scripts(
                &mut dom,
                "https://example.test/page",
            ));
        });
    });

    c.bench_function("request_filter_1000", |b| {
        b.iter(|| {
            let mut blocker = AdBlocker::new(AdBlockConfig::default());
            for index in 0..1_000 {
                black_box(blocker.should_block_resource(
                    &format!("https://cdn{index}.example.test/assets/app.js"),
                    Some("news.example.test"),
                    ResourceType::Script,
                ));
            }
        });
    });
}

criterion_group!(
    benches,
    benchmark_document_pipeline,
    benchmark_javascript,
    benchmark_runtime_and_filtering
);
criterion_main!(benches);
