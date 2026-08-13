//! Isolated GhitaBrowser document preparation worker.

fn main() {
    if let Err(error) = ghitabrowser::worker::run_worker_stdio() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
