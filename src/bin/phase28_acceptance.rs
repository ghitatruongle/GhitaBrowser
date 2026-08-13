use std::path::PathBuf;

use ghitabrowser::acceptance::AcceptanceReleaseManager;

fn main() {
    let mut evidence = None;
    let mut report = None;
    for argument in std::env::args().skip(1) {
        if let Some(path) = argument.strip_prefix("--evidence=") {
            evidence = Some(PathBuf::from(path));
        } else if let Some(path) = argument.strip_prefix("--report=") {
            report = Some(PathBuf::from(path));
        } else {
            eprintln!("Unknown argument: {argument}");
            std::process::exit(2);
        }
    }
    let Some(evidence_path) = evidence else {
        eprintln!("--evidence=<bundle.json> is required");
        std::process::exit(2);
    };
    let Some(report_path) = report else {
        eprintln!("--report=<acceptance-report.json> is required");
        std::process::exit(2);
    };
    let bundle = match AcceptanceReleaseManager::load_bundle(&evidence_path) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("Cannot load evidence: {error}");
            std::process::exit(2);
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let mut manager = AcceptanceReleaseManager::new();
    let acceptance = manager.evaluate_release_acceptance(Some(&bundle), now);
    if let Err(error) = AcceptanceReleaseManager::persist_report(&report_path, &acceptance) {
        eprintln!("Cannot persist acceptance report: {error}");
        std::process::exit(2);
    }
    if acceptance.accepted {
        println!("Phase 28 acceptance passed: {}", report_path.display());
    } else {
        eprintln!("Phase 28 acceptance failed: {}", report_path.display());
        for failure in &acceptance.failures {
            eprintln!("- {failure}");
        }
        std::process::exit(1);
    }
}
