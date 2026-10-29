//! Small CLI branches the main suite leaves out (#40, coverage inventory
//! #15-#17): config validation, `sidebar` without a tty, and the
//! install/dock entry points scripts call (`auto-dock-hook --force`,
//! `fit-sidebar`, `skill install` when it can't link).

mod common;

use common::{FakeHerdrBin, FakeHerdrSocket};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::path::Path;

fn write_config(home: &Path, toml: &str) {
    let dir = common::config_dir(home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

const TAB_PANES: &str = r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t9","terminal_title_stripped":"vim"}]}}"#;
const OPENED: &str = r#"{"result":{"plugin_pane":{"pane":{"pane_id":"w1:p10"}}}}"#;

/// A fake herdr that can dock into tab `w1:t9`.
fn docking_herdr(home: &Path) -> FakeHerdrBin {
    FakeHerdrBin::new(
        home,
        &[
            ("'pane list'*", Some(TAB_PANES)),
            ("'plugin pane open'*", Some(OPENED)),
        ],
    )
}

// --- Config validation ------------------------------------------------------

#[test]
fn a_malformed_config_warns_and_falls_back_to_the_default_feed() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "feed_path = [unclosed\n");
    let feed = common::config_dir(home.path()).join("feed.md");
    std::fs::write(&feed, "# Feed\n\n- [ ] Default item\n").unwrap();

    common::feedr_in(home.path())
        .arg("list")
        .assert()
        .success()
        .stdout(contains("Default item"))
        .stderr(contains("ignoring malformed config.toml"));
}

#[test]
fn an_undockable_width_warns_and_docks_at_the_default_share() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[sidebar]\nauto_dock = true\nwidth = 5.0\n");
    let herdr = docking_herdr(home.path());
    let socket = FakeHerdrSocket::start();

    common::feedr_in(home.path())
        .env("HERDR_BIN_PATH", &herdr.path)
        .env("HERDR_SOCKET_PATH", &socket.path)
        .args(["auto-dock-hook", "--tab-id", "w1:t9"])
        .assert()
        .success()
        .stderr(contains("sidebar.width 5 is not in (0, 1]"));

    let calls = herdr.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.contains("pane resize") && c.contains("--amount 0.18")),
        "{calls:#?}"
    );
}

// --- `feedr sidebar` without a tty -----------------------------------------

#[test]
fn sidebar_without_a_tty_is_a_clean_error_not_a_panic() {
    let home = tempfile::tempdir().unwrap();
    // assert_cmd pipes stdout, so the child never has a terminal.
    common::feedr_in(home.path())
        .arg("sidebar")
        .assert()
        .failure()
        .code(1)
        .stderr(contains("requires an interactive terminal").and(contains("panicked").not()));
}

// --- Install / dock branches ------------------------------------------------

#[test]
fn auto_dock_hook_force_docks_even_with_auto_dock_off() {
    let home = tempfile::tempdir().unwrap();
    // No config: auto_dock defaults to off.
    let herdr = docking_herdr(home.path());
    let socket = FakeHerdrSocket::start();

    common::feedr_in(home.path())
        .env("HERDR_BIN_PATH", &herdr.path)
        .env("HERDR_SOCKET_PATH", &socket.path)
        .args(["auto-dock-hook", "--force", "--tab-id", "w1:t9"])
        .assert()
        .success()
        .stdout(contains("auto-docked"));

    assert!(
        herdr
            .calls()
            .iter()
            .any(|c| c.starts_with("plugin pane open") && c.contains("--target-pane w1:p1")),
        "{:#?}",
        herdr.calls()
    );
}

/// `pane layout` for sidebar `w1:p10` beside `w1:p9` in a 332-column tab.
const LAYOUT: &str = r#"{"result":{"layout":{"tab_id":"w1:t9","area":{"width":332},"panes":[{"pane_id":"w1:p10","rect":{"width":106}},{"pane_id":"w1:p9","rect":{"width":226}}]}}}"#;

#[test]
fn fit_sidebar_sets_the_split_so_the_pane_is_max_width_columns() {
    let home = tempfile::tempdir().unwrap();
    let herdr = FakeHerdrBin::new(home.path(), &[("'pane layout'*", Some(LAYOUT))]);
    let socket = FakeHerdrSocket::with_results(&[(
        "layout.export",
        serde_json::json!({"layout": {"root": {
            "type": "split", "direction": "right", "ratio": 0.32,
            "first": {"type": "pane", "pane_id": "w1:p10"},
            "second": {"type": "pane", "pane_id": "w1:p9"},
        }}}),
    )]);

    common::feedr_in(home.path())
        .env("HERDR_BIN_PATH", &herdr.path)
        .env("HERDR_SOCKET_PATH", &socket.path)
        .args(["fit-sidebar", "--pane", "w1:p10"])
        .assert()
        .success();

    assert_eq!(herdr.calls(), ["pane layout --pane w1:p10"]);
    let reqs = socket.requests();
    assert_eq!(reqs[0], r#"layout.export {"tab_id":"w1:t9"}"#);
    let set: serde_json::Value = serde_json::from_str(
        reqs[1]
            .strip_prefix("layout.set_split_ratio ")
            .unwrap_or_else(|| panic!("{reqs:#?}")),
    )
    .unwrap();
    assert_eq!(set["tab_id"], "w1:t9");
    assert_eq!(set["path"], serde_json::json!([]));
    // Default max_width 46 of 332 columns.
    let ratio = set["ratio"].as_f64().unwrap();
    assert!((ratio * 332.0 - 46.0).abs() < 0.5, "ratio {ratio}");
}

#[test]
fn fit_sidebar_on_a_pane_with_no_side_by_side_split_fails_and_says_so() {
    let home = tempfile::tempdir().unwrap();
    let herdr = FakeHerdrBin::new(home.path(), &[("'pane layout'*", Some(LAYOUT))]);
    let socket = FakeHerdrSocket::with_results(&[(
        "layout.export",
        serde_json::json!({"layout": {"root": {"type": "pane", "pane_id": "w1:p10"}}}),
    )]);

    common::feedr_in(home.path())
        .env("HERDR_BIN_PATH", &herdr.path)
        .env("HERDR_SOCKET_PATH", &socket.path)
        .args(["fit-sidebar", "--pane", "w1:p10"])
        .assert()
        .failure()
        .stderr(contains("not in a side-by-side split"));
    assert!(
        !socket
            .requests()
            .iter()
            .any(|r| r.starts_with("layout.set_split_ratio")),
        "nothing resized"
    );
}

#[test]
fn skill_install_names_the_path_it_could_not_link_and_keeps_the_copy() {
    let home = tempfile::tempdir().unwrap();
    // `~/.claude` is a file, so the skills dir under it can't be created.
    std::fs::write(home.path().join(".claude"), "not a dir").unwrap();

    common::feedr_in(home.path())
        .args(["skill", "install"])
        .assert()
        .failure()
        .stderr(contains(".claude/skills"));

    let copy = home.path().join(".agents/skills/herdr-feedr/SKILL.md");
    assert!(copy.is_file(), "the agent-neutral copy is still written");
}
