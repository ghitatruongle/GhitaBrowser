fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // The JS interpreter runs synchronously on the UI thread with deep
        // call chains (MAX_CALL_DEPTH=64, several KB of frames per level).
        // Raise the main-thread stack reserve so a deep script can't blow the
        // default 2 MB stack at runtime. (Tests still run on their own 2 MB
        // spawn-blocking threads, which is why MAX_CALL_DEPTH stays at 64.)
        if std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "gnu" {
            println!("cargo:rustc-link-arg=-Wl,--stack=23068672"); // 22 MB, GNU ld
        } else {
            println!("cargo:rustc-link-arg=/STACK:23068672"); // 22 MB, MSVC link.exe
        }

        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("Failed to compile Windows icon resource: {}", e);
        }
    }
}
