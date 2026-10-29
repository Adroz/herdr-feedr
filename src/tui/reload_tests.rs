//! A feed reload (watcher event) landing while a modal is open (#40,
//! coverage inventory #14): the modal and what the user typed survive, and
//! the list underneath catches up with disk.

use crate::tui::driver::Driver;
use crate::tui::AppEvent;
use crossterm::event::{KeyCode, KeyModifiers};

fn shows(d: &Driver, text: &str) -> bool {
    d.screen().iter().any(|l| l.contains(text))
}

#[test]
fn a_reload_under_an_open_edit_modal_keeps_the_modal_and_the_typed_text() {
    let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 24);
    d.click(6, 0);
    d.key(KeyCode::Tab, KeyModifiers::NONE); // Category -> Title
    d.type_str(" fix");

    // Another writer adds an item while the modal is open.
    let on_disk = "# Feed\n\n- [ ] Alpha\n- [ ] Beta\n";
    std::fs::write(d.feed_path(), on_disk).unwrap();
    d.push(AppEvent::FeedChanged);

    assert!(
        shows(&d, "Edit item"),
        "modal still open: {:#?}",
        d.screen()
    );
    assert!(shows(&d, "Alpha fix"), "typed text kept: {:#?}", d.screen());

    // Focus and cursor survive too: typing carries on where it left off.
    d.type_str("ed");
    d.key(KeyCode::Char('s'), KeyModifiers::CONTROL);

    assert_eq!(d.feed(), "# Feed\n\n- [ ] Alpha fixed\n- [ ] Beta\n");
    assert!(shows(&d, "[ ] Beta"), "list shows the reloaded item");
}

#[test]
fn a_reload_under_an_open_edit_modal_keeps_the_selection() {
    let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 24);
    d.click(6, 0);
    d.key(KeyCode::Tab, KeyModifiers::NONE); // Category -> Title, cursor at end
    for _ in 0..3 {
        d.key(KeyCode::Left, KeyModifiers::SHIFT);
    }
    let before = d.highlighted();
    assert!(!before.is_empty());

    std::fs::write(d.feed_path(), "# Feed\n\n- [ ] Alpha\n- [ ] Beta\n").unwrap();
    d.push(AppEvent::FeedChanged);

    assert_eq!(
        d.highlighted(),
        before,
        "selection still drawn after the reload"
    );
    d.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(d.clipboard_writes(), ["pha"], "and still the copied span");
}

#[test]
fn cancelling_after_a_reload_writes_nothing_and_shows_the_disk_state() {
    let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 24);
    d.click(6, 0);
    d.key(KeyCode::Tab, KeyModifiers::NONE);
    d.type_str(" fix");

    let on_disk = "# Feed\n\n- [~] Alpha @agent(claude:abc)\n";
    std::fs::write(d.feed_path(), on_disk).unwrap();
    d.push(AppEvent::FeedChanged);
    d.key(KeyCode::Esc, KeyModifiers::NONE);

    assert!(!shows(&d, "Edit item"));
    assert_eq!(d.feed(), on_disk, "Esc discards; the reload wrote nothing");
    assert!(shows(&d, "[~] Alpha"), "{:#?}", d.screen());
}

#[test]
fn the_file_view_shows_the_reloaded_feed() {
    let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 24);
    d.key(KeyCode::Char('f'), KeyModifiers::NONE);
    assert!(shows(&d, "- [ ] Alpha"), "{:#?}", d.screen());

    std::fs::write(d.feed_path(), "# Feed\n\n- [ ] Alpha\n- [ ] Gamma\n").unwrap();
    d.push(AppEvent::FeedChanged);

    assert!(
        shows(&d, "feed file"),
        "viewer still open: {:#?}",
        d.screen()
    );
    assert!(shows(&d, "- [ ] Gamma"), "{:#?}", d.screen());
}
