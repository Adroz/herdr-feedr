//! The plugin's shell wrappers (scripts/*.sh), each run against a fake
//! `herdr` that records argv, with the real `feedr` binary behind them.

mod common;

use assert_cmd::Command;
use common::{FakeHerdrBin, FakeHerdrSocket};
use predicates::str::contains;
use std::path::Path;

/// `herdr pane list` output, keys sorted the way herdr prints them.
fn pane_list(panes: &[(&str, &str, &str, bool)]) -> String {
    let entries: Vec<String> = panes
        .iter()
        .map(|(pane, tab, title, focused)| {
            format!(
                r#"{{"focused":{focused},"pane_id":"{pane}","tab_id":"{tab}","terminal_title_stripped":"{title}","workspace_id":"{}"}}"#,
                pane.split(':').next().unwrap()
            )
        })
        .collect();
    format!(r#"{{"result":{{"panes":[{}]}}}}"#, entries.join(","))
}

const OPENED: &str = r#"{"result":{"plugin_pane":{"pane":{"pane_id":"w1:p10"}}}}"#;

fn write_config(home: &Path, toml: &str) {
    let dir = common::config_dir(home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

/// `bash scripts/<name>` in a hermetic env wired to the fakes.
fn script(name: &str, home: &Path, herdr: &FakeHerdrBin, socket: &FakeHerdrSocket) -> Command {
    let mut cmd = Command::new("bash");
    common::hermetic(&mut cmd, home);
    cmd.arg(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join(name),
    )
    .env("HERDR_BIN_PATH", &herdr.path)
    .env("HERDR_SOCKET_PATH", &socket.path)
    .env("FEEDR_BIN_PATH", assert_cmd::cargo::cargo_bin("feedr"));
    cmd
}

#[test]
fn open_feedr_docks_through_feedr_on_the_configured_side_and_width() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[sidebar]\nside = \"right\"\nwidth = 0.25\n");
    let list = pane_list(&[
        ("w1:p2", "w1:t1", "zsh", false),
        ("w1:p1", "w1:t1", "vim", true),
    ]);
    let herdr = FakeHerdrBin::new(
        home.path(),
        &[
            ("'pane list'*", Some(&list)),
            ("'plugin pane open'*", Some(OPENED)),
        ],
    );
    let socket = FakeHerdrSocket::start();

    script("open-feedr.sh", home.path(), &herdr, &socket)
        .assert()
        .success();

    assert_eq!(
        herdr.calls(),
        [
            "pane list",
            "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
             --placement split --direction right --target-pane w1:p1",
            "pane resize --direction right --amount 0.25 --pane w1:p10",
            "pane layout --pane w1:p10",
        ]
    );
}

#[test]
fn open_feedr_focuses_a_sidebar_already_open_in_another_workspace() {
    let home = tempfile::tempdir().unwrap();
    let list = pane_list(&[
        ("w1:p1", "w1:t1", "vim", true),
        ("w2:p3", "w2:t4", "feedr-sidebar", false),
    ]);
    let herdr = FakeHerdrBin::new(home.path(), &[("'pane list'*", Some(&list))]);
    let socket = FakeHerdrSocket::start();

    script("open-feedr.sh", home.path(), &herdr, &socket)
        .assert()
        .success();

    assert_eq!(herdr.calls(), ["pane list"], "nothing new opened");
    assert_eq!(socket.requests(), [r#"pane.focus {"pane_id":"w2:p3"}"#]);
}

#[test]
fn toggle_feedr_closes_an_open_sidebar() {
    let home = tempfile::tempdir().unwrap();
    let list = pane_list(&[
        ("w1:p1", "w1:t1", "vim", true),
        ("w1:p3", "w1:t1", "feedr-sidebar", false),
    ]);
    let herdr = FakeHerdrBin::new(home.path(), &[("'pane list'*", Some(&list))]);
    let socket = FakeHerdrSocket::start();

    script("toggle-feedr.sh", home.path(), &herdr, &socket)
        .assert()
        .success();

    assert_eq!(herdr.calls(), ["pane list", "plugin pane close w1:p3"]);
}

#[test]
fn toggle_feedr_opens_a_docked_sidebar_when_none_is_open() {
    let home = tempfile::tempdir().unwrap();
    let list = pane_list(&[("w1:p1", "w1:t1", "vim", true)]);
    let herdr = FakeHerdrBin::new(
        home.path(),
        &[
            ("'pane list'*", Some(&list)),
            ("'plugin pane open'*", Some(OPENED)),
            ("'pane neighbor'*", None), // already at the left edge
        ],
    );
    let socket = FakeHerdrSocket::start();

    script("toggle-feedr.sh", home.path(), &herdr, &socket)
        .assert()
        .success();

    assert_eq!(
        herdr.calls(),
        [
            "pane list", // toggle's own check
            "pane list", // feedr sidebar --dock's
            "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
             --placement split --direction right --target-pane w1:p1",
            "pane neighbor --direction left --pane w1:p10",
            "pane resize --direction left --amount 0.18 --pane w1:p10",
            "pane layout --pane w1:p10",
        ]
    );
}

/// The hook reads the new tab from HERDR_TAB_ID, docks into it without
/// focus, and hands focus back to the pane the user was in.
#[test]
fn on_tab_created_docks_into_the_new_tab_and_restores_focus() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[sidebar]\nauto_dock = true\n");
    let list = pane_list(&[
        ("w1:p1", "w1:t1", "vim", true),
        ("w1:p9", "w1:t9", "zsh", false),
    ]);
    let herdr = FakeHerdrBin::new(
        home.path(),
        &[
            ("'pane list'*", Some(&list)),
            ("'plugin pane open'*", Some(OPENED)),
            ("'pane neighbor'*", None),
        ],
    );
    let socket = FakeHerdrSocket::start();

    script("on-tab-created.sh", home.path(), &herdr, &socket)
        .env("HERDR_TAB_ID", "w1:t9")
        .assert()
        .success()
        .stdout(contains("auto-docked"));

    assert_eq!(
        herdr.calls(),
        [
            "pane list",
            "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
             --placement split --direction right --target-pane w1:p9 --no-focus",
            "pane neighbor --direction left --pane w1:p10",
            "pane resize --direction left --amount 0.18 --pane w1:p10",
            "pane layout --pane w1:p10",
        ]
    );
    assert_eq!(socket.requests(), [r#"pane.focus {"pane_id":"w1:p1"}"#]);
}

#[test]
fn on_tab_created_falls_back_to_the_event_json_for_the_tab_id() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[sidebar]\nauto_dock = true\n");
    let list = pane_list(&[("w1:p9", "w1:t9", "feedr-sidebar", true)]);
    let herdr = FakeHerdrBin::new(home.path(), &[("'pane list'*", Some(&list))]);
    let socket = FakeHerdrSocket::start();

    script("on-tab-created.sh", home.path(), &herdr, &socket)
        .env(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"event":"tab_created","data":{"tab_id":"w1:t9"}}"#,
        )
        .assert()
        .success()
        .stdout(contains("tab w1:t9 already has sidebar pane w1:p9"));

    assert_eq!(herdr.calls(), ["pane list"]);
}

/// In place: split a fresh sidebar off the old one, close the old, fit the
/// new one, then put each workspace's active tab back.
#[test]
fn reload_feedr_replaces_each_sidebar_in_place() {
    let home = tempfile::tempdir().unwrap();
    let list = pane_list(&[
        ("w1:p1", "w1:t1", "vim", true),
        ("w1:p3", "w1:t1", "feedr-sidebar", false),
    ]);
    let workspaces = r#"{"result":{"workspaces":[{"active_tab_id":"w1:t1","focused":true,"workspace_id":"w1"}]}}"#;
    let herdr = FakeHerdrBin::new(
        home.path(),
        &[
            ("'pane list'*", Some(&list)),
            ("'workspace list'*", Some(workspaces)),
            ("'plugin pane open'*", Some(OPENED)),
        ],
    );
    let socket = FakeHerdrSocket::start();

    script("reload-feedr.sh", home.path(), &herdr, &socket)
        .arg("--no-build")
        .assert()
        .success()
        .stdout(contains("ok     w1:t1: replaced w1:p3 with w1:p10"));

    assert_eq!(
        herdr.calls(),
        [
            "pane list",
            "workspace list",
            "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
             --placement split --direction right --target-pane w1:p3 --no-focus",
            "pane close w1:p3",
            "pane layout --pane w1:p10", // feedr fit-sidebar
            "tab focus w1:t1",
        ]
    );
}
