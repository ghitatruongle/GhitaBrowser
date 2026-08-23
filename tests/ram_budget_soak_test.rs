use ghitabrowser::memory_tracker::MemoryBudget;
use ghitabrowser::parser::parse_html;
use ghitabrowser::tab::{HistoryEntry, WakeResult};
use ghitabrowser::Browser;

const MB: usize = 1024 * 1024;

#[test]
fn twenty_tab_session_stays_bounded_and_protects_important_tabs() {
    let mut browser = Browser::new_in_memory();
    let html = format!(
        "<main><h1>Memory fixture</h1>{}</main>",
        (0..1_000)
            .map(|index| format!("<p>tab content row {index} remains readable after wake</p>"))
            .collect::<String>()
    );
    let mut ids = Vec::new();
    for index in 0..20 {
        let id = browser.add_tab(
            &format!("https://site{index}.test/page"),
            parse_html(&html),
            &format!("Tab {index}"),
        );
        if let Some(tab) = browser.active_tab_mut() {
            tab.last_active_timestamp -= 7_200 + index as i64;
            if index == 0 {
                tab.is_pinned = true;
            }
            if index == 1 {
                tab.is_audible = true;
            }
        }
        ids.push(id);
    }
    let pinned = ids[0];
    let audible = ids[1];
    let active = browser.active_tab().unwrap().id;
    let before = browser.estimate_memory().total_bytes;
    let budget = MemoryBudget::from_bytes(before / 3, before / 2);
    let mut discarded = 0usize;
    for _ in 0..10 {
        let report = browser.relieve_memory_pressure(budget, 2);
        assert!(report.discarded_tabs.len() <= 3);
        discarded += report.discarded_tabs.len();
        if report.after_bytes < budget.hard_limit_bytes {
            break;
        }
    }
    let after = browser.estimate_memory().total_bytes;
    assert!(after < before, "before={before}, after={after}");
    assert!(after <= 500 * MB);
    assert!(!browser.is_tab_discarded(active));
    assert!(!browser.is_tab_discarded(pinned));
    assert!(!browser.is_tab_discarded(audible));
    assert!(discarded <= 18);
}

#[test]
fn one_thousand_navigations_keep_history_bounded() {
    let mut browser = Browser::new_in_memory();
    browser.add_tab(
        "https://history.test/0",
        parse_html("<main><p>initial page</p></main>"),
        "History",
    );
    let mut steady_bytes = 0usize;
    for index in 1..=1_000 {
        let dom = parse_html(&format!("<main><p>page {index}</p></main>"));
        let tab = browser.active_tab_mut().unwrap();
        tab.push_history(HistoryEntry::new(
            format!("https://history.test/{index}"),
            format!("Page {index}"),
            &dom,
        ));
        if index == 100 {
            steady_bytes = tab.history_retained_bytes();
        }
    }
    let tab = browser.active_tab().unwrap();
    assert_eq!(tab.history_len(), 60);
    assert!(tab.history_retained_bytes() <= steady_bytes + steady_bytes / 5);
}

#[test]
fn sleep_wake_and_discard_keep_the_restore_contract() {
    let mut browser = Browser::new_in_memory();
    let id = browser.add_tab(
        "https://restore.test/page",
        parse_html("<main><h1>Readable restored page</h1><p>State survives sleep.</p></main>"),
        "Restore",
    );
    let before = browser.estimate_tab_memory(id).unwrap().total_bytes;
    browser.active_tab_mut().unwrap().sleep();
    let sleeping = browser.estimate_tab_memory(id).unwrap().total_bytes;
    assert!(sleeping < before);
    assert_eq!(browser.wake_tab(id), WakeResult::RestoredFromCache);
    assert!(browser.render_current().contains("Readable restored page"));

    browser.active_tab_mut().unwrap().discard();
    assert_eq!(
        browser.undiscard_tab(id),
        Some("https://restore.test/page".to_string())
    );
}
