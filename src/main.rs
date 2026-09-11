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
            let executable = match std::env::current_exe() {
                Ok(path) => path,
                Err(error) => {
                    log::error!("Cannot locate current executable: {error}");
                    std::process::exit(2);
                }
            };
            let outcome = ghitabrowser::release_smoke::run(&executable);
            let report = match &outcome {
                Ok(report) => report.clone(),
                Err(error) => serde_json::json!({
                    "passed": false,
                    "version": ghitabrowser::VERSION,
                    "error": error
                }),
            };
            let report_bytes = match serde_json::to_vec_pretty(&report) {
                Ok(bytes) => bytes,
                Err(error) => {
                    log::error!("Cannot serialize release-smoke report: {error}");
                    std::process::exit(2);
                }
            };
            if std::fs::write(report_path, report_bytes).is_err() {
                log::error!("Cannot write release-smoke report to {report_path}");
                std::process::exit(2);
            }
            std::process::exit(if outcome.is_ok() { 0 } else { 1 });
        }
        argument
    });
    if let Err(error) = ghitabrowser::ui::run_gui_with_target(initial_target) {
        log::error!("GhitaBrowser GUI failed: {error}");
        #[cfg(not(debug_assertions))]
        {
            // Release builds have no console (`windows_subsystem = "windows"`),
            // so a swallowed `let _ = error` would vanish. Persist the GUI
            // failure to a log file next to the temp dir for diagnostics.
            let log_path = std::env::temp_dir().join("ghitabrowser-gui-error.log");
            let _ = std::fs::write(&log_path, format!("GhitaBrowser GUI failed: {error}\n"));
        }
    }
}
