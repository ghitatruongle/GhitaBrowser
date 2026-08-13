use std::sync::{Arc, Mutex};

use ghitabrowser::{DefaultAction, DomEvent, ListenerOptions, LiveDocument};

#[test]
fn phase11_mutations_incrementally_refresh_pixels_and_accessibility() {
    let mut document = LiveDocument::parse(
        "<main id='app'><button id='save' type='button'>Save</button><input id='name'><p id='status'>waiting</p></main>",
        Vec::new(),
        800,
    );
    let status = document.get_element_by_id("status").unwrap();
    let initial_revision = document.render_state().revision;
    document
        .set_text_content(status, "saved successfully")
        .unwrap();
    let snapshot = document.refresh();
    assert!(snapshot.revision > initial_revision);
    assert!(snapshot.display_list.items.iter().any(|item| matches!(
        item,
        ghitabrowser::DisplayItem::TextRun { content, .. } if content.contains("saved successfully")
    )));
    assert!(snapshot
        .accessibility
        .root
        .as_ref()
        .is_some_and(|root| format!("{root:?}").contains("saved successfully")));

    let app = document.get_element_by_id("app").unwrap();
    let save = document.get_element_by_id("save").unwrap();
    let phases = Arc::new(Mutex::new(Vec::new()));
    let phases_for_capture = phases.clone();
    document
        .add_event_listener(
            app,
            "click",
            ListenerOptions {
                capture: true,
                ..Default::default()
            },
            Arc::new(move |event| phases_for_capture.lock().unwrap().push(event.phase)),
        )
        .unwrap();
    let phases_for_target = phases.clone();
    document
        .add_event_listener(
            save,
            "click",
            ListenerOptions::default(),
            Arc::new(move |event| phases_for_target.lock().unwrap().push(event.phase)),
        )
        .unwrap();
    let report = document.click(save).unwrap();
    assert_eq!(report.invoked_listeners, 2);
    assert_eq!(
        phases.lock().unwrap().as_slice(),
        &[
            ghitabrowser::EventPhase::Capturing,
            ghitabrowser::EventPhase::AtTarget
        ]
    );

    let name = document.get_element_by_id("name").unwrap();
    document.focus(name).unwrap();
    let input = document.dispatch_keyboard("keydown", "G").unwrap();
    assert_eq!(document.get_attribute(name, "value"), Some("G"));
    assert!(input
        .default_actions
        .contains(&DefaultAction::InsertText(name, "G".into())));

    let mut custom = DomEvent::new("click", save);
    custom.prevent_default();
    assert!(custom.default_prevented);
}
