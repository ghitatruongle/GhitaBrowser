// Integration tests for Phase 25 — Cosmetic Content Control & HTTPS-Only Upgrades.

use ghitabrowser::content_control::ContentControlEngine;
use ghitabrowser::https_upgrade::{HttpsMode, HttpsUpgradeEngine, HttpsUpgradeResult};

#[test]
fn cosmetic_filtering_css_generation_per_origin() {
    let mut engine = ContentControlEngine::new();
    engine.add_rule(".ad-container", None);
    engine.add_rule("#sidebar-ad", Some("news.com".to_string()));

    let news_css = engine.generate_cosmetic_css_for_origin("https://news.com/article");
    assert!(news_css.contains(".ad-container"));
    assert!(news_css.contains("#sidebar-ad"));

    let blog_css = engine.generate_cosmetic_css_for_origin("https://blog.org/post");
    assert!(blog_css.contains(".ad-container"));
    assert!(!blog_css.contains("#sidebar-ad"));
}

#[test]
fn https_only_mode_automatic_upgrades() {
    let engine = HttpsUpgradeEngine::new(HttpsMode::EnabledAll);

    // Standard HTTP upgrades to HTTPS
    assert_eq!(
        engine.evaluate_url("http://shop.com/checkout"),
        HttpsUpgradeResult::Upgraded {
            new_url: "https://shop.com/checkout".to_string()
        }
    );

    // HTTPS remains secure
    assert_eq!(
        engine.evaluate_url("https://bank.com"),
        HttpsUpgradeResult::AlreadySecure {
            url: "https://bank.com".to_string()
        }
    );

    // Localhost exemption
    assert_eq!(
        engine.evaluate_url("http://127.0.0.1:8000"),
        HttpsUpgradeResult::ExemptLocal {
            url: "http://127.0.0.1:8000".to_string()
        }
    );
}
