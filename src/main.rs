// GhitaBrowser v0.6.1 - Main entry point
#![windows_subsystem = "windows"]

#[cfg(debug_assertions)]
use ghitabrowser::css_parser::parse_css;
#[cfg(debug_assertions)]
use ghitabrowser::javascript::JsvEngine;
#[cfg(debug_assertions)]
use ghitabrowser::layout;
#[cfg(debug_assertions)]
use ghitabrowser::network::{fetch_url, fetch_with_cache, FetchResult, ResourceCache};
#[cfg(debug_assertions)]
use ghitabrowser::parser::parse_html;
#[cfg(debug_assertions)]
use ghitabrowser::performance::Profiler;
#[cfg(debug_assertions)]
use ghitabrowser::storage::{Cookie, CookieStore};
#[cfg(debug_assertions)]
use ghitabrowser::text_renderer::TextRenderer;
#[cfg(debug_assertions)]
use ghitabrowser::Browser;
#[cfg(debug_assertions)]
use std::collections::HashMap;

fn print_banner() {
    #[cfg(debug_assertions)]
    {
        println!("╔══════════════════════════════════════════╗");
        println!(
            "║   🦀 GhitaBrowser {:<22} ║",
            format!("v{}", ghitabrowser::VERSION)
        );
        println!("║   Next-Gen Rust Browser Engine           ║");
        println!("╚══════════════════════════════════════════╝");
    }
}

fn main() {
    // Initialize logging (only in debug mode)
    #[cfg(debug_assertions)]
    {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }

    print_banner();

    // ========== 1. Real HTTP Networking ==========
    #[cfg(debug_assertions)]
    {
        println!("\n📡 Network: Real HTTP/HTTPS via ureq");
        match fetch_url("https://httpbin.org/get") {
            Ok(result) => {
                println!(
                    "   ✅ Fetched: {} ({} bytes, {} ms, status {})",
                    result.url,
                    result.body.len(),
                    result.fetch_time_ms,
                    result.status_code
                );
            }
            Err(e) => {
                println!("   ⚠️  Network test: {} (offline?)", e);
            }
        }
    }

    // ========== 2. HTML Parsing ==========
    #[cfg(debug_assertions)]
    {
        println!("\n📝 HTML Parser: Error recovery & HTML5 support");
        let test_html = r#"<html>
            <head><title>GhitaBrowser Test</title></head>
            <body>
                <h1 class="main-title">Welcome to GhitaBrowser v0.6.1!</h1>
                <p>This is a <strong>Rust</strong> browser built from scratch.</p>
                <img src="logo.png" alt="Logo">
                <!-- This is a comment -->
                <script>var x = 1 < 2;</script>
                <ul>
                    <li>Item A &amp; B</li>
                    <li>Item C</li>
                </ul>
            </body>
        </html>"#;

        let dom = parse_html(test_html);
        println!("   ✅ Parsed HTML successfully");
        if let Some(title) = dom.find_tag("title") {
            println!("   📌 Title: {}", title.text);
        }
        if let Some(h1) = dom.find_tag("h1") {
            println!("   📌 H1: {}", h1.text);
        }
        if let Some(script) = dom.find_tag("script") {
            println!(
                "   📌 Script tag content preserved: {} chars",
                script.text.len()
            );
        }
    }

    // ========== 3. CSS Styling ==========
    #[cfg(debug_assertions)]
    {
        println!("\n🎨 CSS Engine: Selectors with specificity");
        let css = r#"
            h1 { color: navy; font-size: 28px; margin: 10px; }
            .main-title { color: darkblue; font-weight: bold; }
            p { font-size: 16px; line-height: 1.5; color: #333; }
            ul { margin: 20px; padding: 10px; }
            li { color: #555; }
        "#;
        let css_rules = parse_css(css);
        println!("   ✅ Parsed {} CSS rules", css_rules.len());
        for rule in &css_rules {
            let sel_str: Vec<String> = rule
                .selectors
                .iter()
                .map(|s| {
                    let mut parts = Vec::new();
                    if let Some(ref tag) = s.tag {
                        parts.push(tag.clone());
                    }
                    if let Some(ref class) = s.class {
                        parts.push(format!(".{}", class));
                    }
                    if let Some(ref id) = s.id {
                        parts.push(format!("#{}", id));
                    }
                    parts.join("")
                })
                .collect();
            println!(
                "   📐 {} => {} declarations (specificity: {:?})",
                sel_str.join(", "),
                rule.declarations.len(),
                rule.specificity
            );
        }
    }

    // ========== 4. Layout Engine ==========
    #[cfg(debug_assertions)]
    {
        println!("\n📐 Layout Engine: Box model with text wrapping");
        let test_html = r#"<html><head><title>GhitaBrowser Test</title></head><body>
            <h1 class="main-title">Welcome to GhitaBrowser v0.6.1!</h1>
            <p>This is a <strong>Rust</strong> browser built from scratch.</p>
            <ul><li>Item A &amp; B</li><li>Item C</li></ul>
        </body></html>"#;
        let dom = parse_html(test_html);
        let css_rules = parse_css(
            r#"h1 { color: navy; font-size: 28px; } .main-title { font-weight: bold; } p { font-size: 16px; } ul { margin: 20px; }"#,
        );
        if let Some(mut layout_tree) = layout::create_layout_tree(&dom, &css_rules, 800) {
            layout::perform_layout(&mut layout_tree, 800.0);
            let tr = TextRenderer::new(800, 600);
            let out = tr.render_to_text(&layout_tree);
            println!("\n--- Rendered Web Content ---");
            for line in out.lines().take(25) {
                println!("{}", line);
            }
            if out.lines().count() > 25 {
                println!("... ({} more lines)", out.lines().count() - 25);
            }
        }
    }

    // ========== 5. JavaScript Engine ==========
    #[cfg(debug_assertions)]
    {
        println!("\n⚡ JavaScript Engine: Variables, functions, control flow");
        let mut js_engine = JsvEngine::new();
        if let Ok(val) = js_engine.eval("1 + 1") {
            println!("   ✅ 1 + 1 = {}", val.to_display_string());
        }

        let _ = js_engine.eval("let x = 42");
        if let Ok(val) = js_engine.eval("x * 2") {
            println!("   ✅ let x = 42; x * 2 = {}", val.to_display_string());
        }

        let _ = js_engine.eval("function add(a, b) { return a + b; }");
        if let Ok(val) = js_engine.eval("add(10, 20)") {
            println!("   ✅ add(10, 20) = {}", val.to_display_string());
        }

        let _ = js_engine.eval("let i = 0; while (i < 3) { i = i + 1; }");
        if let Ok(val) = js_engine.eval("i") {
            println!("   ✅ while loop: i = {}", val.to_display_string());
        }
    }

    // ========== 6. Storage System ==========
    #[cfg(debug_assertions)]
    {
        println!("\n💾 Storage: Persistent cookies & localStorage");
        let mut browser = Browser::new();
        browser.storage.cookies_mut().add_cookie(Cookie::new(
            "session",
            "abc123",
            ".ghitabrowser.local",
            "/",
        ));
        browser.storage.cookies_mut().add_cookie(Cookie::new(
            "theme",
            "dark",
            ".ghitabrowser.local",
            "/",
        ));
        {
            let ls = browser.storage.local_storage("https://ghitabrowser.local");
            ls.set("user_pref", "fullscreen");
            ls.set("last_visit", "2026-07-30");
        }
        println!(
            "   ✅ Cookies: {}, localStorage items: {}",
            browser.storage.cookie_count(),
            browser.storage.local_storage_count()
        );
    }

    // ========== 7. Resource Cache with TTL ==========
    #[cfg(debug_assertions)]
    {
        println!("\n📦 Resource Cache: TTL-based with stats");
        let mut cache = ResourceCache::new();
        let mut cookie_store = CookieStore::new();
        let test_url = "https://httpbin.org/html";
        match fetch_with_cache(test_url, &mut cache, Some(&mut cookie_store)) {
            Ok(result) => {
                println!(
                    "   ✅ Cached: {} ({} bytes, {} ms)",
                    result.url,
                    result.body.len(),
                    result.fetch_time_ms
                );
                if let Ok(cached) = fetch_with_cache(test_url, &mut cache, Some(&mut cookie_store))
                {
                    println!("   ✅ Cache hit: {} (from cache)", cached.url);
                }
            }
            Err(e) => {
                println!("   ⚠️  Cache test: {} (offline?)", e);
                let result = FetchResult {
                    body: String::from("<html><body><h1>Offline</h1></body></html>"),
                    url: test_url.to_string(),
                    status_code: 200,
                    content_type: "text/html".to_string(),
                    headers: HashMap::new(),
                    fetch_time_ms: 0,
                    set_cookie_headers: vec![],
                };
                cache.insert(test_url, result, 300);
                println!("   ✅ Offline cache: stored test data");
            }
        }
    }

    // ========== 8. Performance Profiler ==========
    #[cfg(debug_assertions)]
    {
        println!("\n⏱️  Performance Profiler");
        let mut profiler = Profiler::new();
        profiler.record("fetch", 45);
        profiler.record("parse", 12);
        profiler.record("style", 3);
        profiler.record("layout", 8);
        profiler.record("render", 5);
        profiler.report();
    }

    // ========== 9. Summary ==========
    #[cfg(debug_assertions)]
    {
        println!("\n═══════════════════════════════════════════");
        println!("✅ All subsystems initialized successfully!");
        println!(
            "🌐 Launching GhitaBrowser v{} GUI...",
            ghitabrowser::VERSION
        );
        println!("═══════════════════════════════════════════\n");
    }

    // Launch GUI
    match ghitabrowser::ui::run_gui() {
        Ok(()) => {}
        #[cfg(debug_assertions)]
        Err(e) => eprintln!("GUI launch error: {}", e),
        #[cfg(not(debug_assertions))]
        Err(_) => {}
    }
}
