fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // The JS interpreter runs synchronously on the UI thread with deep
        // call chains (MAX_CALL_DEPTH is bounded, with several KB of frames per level).
        // Raise the main-thread stack reserve so a deep script can't blow the
        // default 2 MB stack at runtime. (Tests still run on their own 2 MB
        // spawn-blocking threads, which is why MAX_CALL_DEPTH stays at 64.)
        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        if target_env == "gnu" {
            println!("cargo:rustc-link-arg=-Wl,--stack=23068672"); // 22 MB, GNU ld
        } else {
            println!("cargo:rustc-link-arg=/STACK:23068672"); // 22 MB, MSVC link.exe
        }

        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Ok(version) = std::env::var("CARGO_PKG_VERSION") {
            res.set("ProductName", "GhitaBrowser");
            res.set("FileDescription", "GhitaBrowser document browser");
            res.set("ProductVersion", &version);
            res.set("FileVersion", &version);
        }
        if let Err(error) = res.compile() {
            panic!("failed to compile Windows resources: {error}");
        }

        // GNU ld can discard winres' static archive because a resource object
        // has no referenced code symbol. Link the generated object explicitly
        // so icon and version metadata survive dead-code elimination/LTO.
        if target_env == "gnu" {
            let resource_object = std::path::PathBuf::from(
                std::env::var("OUT_DIR").expect("Cargo must set OUT_DIR for build scripts"),
            )
            .join("resource.o");
            println!("cargo:rustc-link-arg={}", resource_object.display());
        }
    }
}
