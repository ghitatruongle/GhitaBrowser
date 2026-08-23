//! GhitaBrowser desktop entry point.

#![windows_subsystem = "windows"]

fn main() {
    #[cfg(debug_assertions)]
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match ghitabrowser::compatibility_probe::try_run_cli(&args) {
        Ok(Some(passed)) => std::process::exit(if passed { 0 } else { 2 }),
        Err(error) => {
            log::error!("Compatibility probe failed: {error}");
            std::process::exit(2);
        }
        Ok(None) => {}
    }

    let initial_target = std::env::args_os().nth(1).map(|argument| {
        let argument = argument.to_string_lossy().into_owned();
        if let Some(report_path) = argument.strip_prefix("--release-smoke-report=") {
            let executable = std::env::current_exe().unwrap_or_default();
            let outcome = ghitabrowser::release_smoke::run(&executable);
            let report = match &outcome {
                Ok(report) => report.clone(),
                Err(error) => serde_json::json!({
                    "passed": false,
                    "version": ghitabrowser::VERSION,
                    "error": error
                }),
            };
            if std::fs::write(
                report_path,
                serde_json::to_vec_pretty(&report).unwrap_or_default(),
            )
            .is_err()
            {
                std::process::exit(2);
            }
            std::process::exit(if outcome.is_ok() { 0 } else { 1 });
        }
        argument
    });
    if let Err(error) = ghitabrowser::ui::run_gui_with_target(initial_target) {
        #[cfg(debug_assertions)]
        log::error!("GhitaBrowser GUI failed: {error}");
        #[cfg(not(debug_assertions))]
        let _ = error;
    }
}
