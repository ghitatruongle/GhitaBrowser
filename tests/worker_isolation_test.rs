use std::path::PathBuf;
use std::time::Duration;

use ghitabrowser::worker::{
    prepare_pdf_with_program, prepare_with_program, prepare_with_program_cancellable,
    PreparationRequest, WorkerCancellationToken, WorkerError,
};

#[test]
fn isolated_worker_prepares_document() {
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_ghita-renderer-worker"));
    let request = PreparationRequest {
        html: "<html><head><title>Worker</title></head><body><h1>Hello</h1></body></html>"
            .to_string(),
        fallback_title: "fallback".to_string(),
        base_rules: Vec::new(),
        viewport_width: 800,
        viewport_height: 600,
    };
    let prepared = prepare_with_program(&worker, &request, Duration::from_secs(5)).unwrap();
    assert_eq!(prepared.title, "Worker");
    assert!(prepared.rendered_text.contains("Hello"));
    assert!(prepared.layout.is_some());
}

#[test]
fn isolated_worker_handles_unicode_scripts_without_crashing() {
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_ghita-renderer-worker"));
    let request = PreparationRequest {
        html: "<html><head><title>Unicode</title></head><body><script>let message='Xin vui l\u{1eaf}ng'; message == 'Xin vui l\u{1eaf}ng'</script><p>Ready</p></body></html>".to_string(),
        fallback_title: "unicode".to_string(),
        base_rules: Vec::new(),
        viewport_width: 1024,
        viewport_height: 768,
    };
    let prepared = prepare_with_program(&worker, &request, Duration::from_secs(10))
        .expect("Unicode script must not crash the renderer worker");
    assert_eq!(prepared.title, "Unicode");
    assert!(prepared.layout.is_some());
}

#[test]
fn isolated_worker_prepares_pdf() {
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_ghita-renderer-worker"));
    let pdf = b"%PDF-1.7\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R >> endobj\n\
        4 0 obj << /Length 27 >> stream\nBT (Worker PDF) Tj ET\nendstream endobj\n%%EOF";
    let prepared = prepare_pdf_with_program(
        &worker,
        pdf,
        "worker.pdf",
        &[],
        800,
        600,
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(prepared.title.contains("worker.pdf"));
    assert!(prepared.rendered_text.contains("Worker PDF"));
}

#[test]
fn live_worker_is_terminated_when_navigation_is_cancelled() {
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_ghita-renderer-worker"));
    let request = PreparationRequest {
        html: format!(
            "<html><body>{}</body></html>",
            "<div>bounded cancellation payload</div>".repeat(100_000)
        ),
        fallback_title: "cancelled".to_string(),
        base_rules: Vec::new(),
        viewport_width: 1_280,
        viewport_height: 720,
    };
    let cancellation = WorkerCancellationToken::new();
    let child_cancellation = cancellation.clone();
    let operation = std::thread::spawn(move || {
        prepare_with_program_cancellable(
            &worker,
            &request,
            Duration::from_secs(15),
            &child_cancellation,
        )
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !cancellation.worker_started() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        cancellation.worker_started(),
        "fault injection must observe a live isolated worker"
    );
    cancellation.cancel();

    let result = operation.join().expect("worker controller must not panic");
    assert!(matches!(result, Err(WorkerError::Cancelled)));
}
