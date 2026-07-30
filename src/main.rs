#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod parser;
mod css_parser;
mod layout;
mod text_renderer;
mod renderer;
mod image_loader;
mod ui;
mod window;
mod tab;
mod storage;
mod javascript;
mod performance;

use parser::Element;
use tab::TabManager;
use storage::{CookieStore, StorageManager};
use javascript::JsvEngine;
use performance::Profiler;

fn main() {
    println!("♻ GhitaBrowser v0.0.0 - Starting GUI Application");

    // ========== Core Engine Pipeline ==========
    
    let test_html = "<html><body><h1>Welcome to GhitaBrowser v0.0.0!</h1><p>This is a Rust browser built from scratch.</p></body></html>";
    let dom = parser::parse_html(test_html);

    let css_rules: Vec<_> = vec![];
    if let Some(mut layout_tree) = layout::create_layout_tree(&dom, &css_rules, 1024) {
        layout::perform_layout(&mut layout_tree, 1024);
        let tr = text_renderer::TextRenderer::new(1024, 768);
        let out = tr.render_to_text(&layout_tree);
        println!("\n--- Rendered Web Content ---");
        println!("{}", out);
    }

    // ========== Subsystems Demo ==========
    
    println!("--- Tab Management System ---");
    let mut tm = TabManager::new();
    let _id1 = tm.add_tab("https://google.com", Element::new("body"), "Google");
    let _id2 = tm.add_tab("https://github.com", Element::new("div"), "GitHub");
    println!("✅ Active tabs: {}", tm.tab_count());

    println!("\n--- Storage System ---");
    let mut cookie_store = CookieStore::new();
    let cookie1 = storage::Cookie::new("session", "abc123", ".example.com", "/");
    cookie_store.add_cookie(cookie1);
    let mut storage = StorageManager::new();
    let _local = storage.local_storage("https://example.com");
    println!("✅ Storage manager initialized");

    println!("\n--- JavaScript Engine ---");
    let mut js_engine = JsvEngine::new();
    if let Ok(val) = js_engine.eval("1 + 1") {
        if let Some(n) = val.as_number() {
            println!("✅ JavaScript Evaluator: 1 + 1 = {}", n);
        }
    }

    println!("\n--- Performance Optimization ---");
    let mut profiler = Profiler::new();
    profiler.record("parse", 25);
    profiler.record("layout", 5);
    profiler.report();

    println!("\n🌐 Launching Interactive GhitaBrowser v0.0.0 GUI Window...");

    if let Err(e) = ui::run_gui() {
        eprintln!("GUI launch error: {}", e);
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
    }
}