use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn feedr(feed: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.arg("--file").arg(feed);
    cmd
}

fn seed(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("feed.md");
    std::fs::write(
        &path,
        "\
# Feed

- [ ] Fix auth redirect loop
  Repro: bounces forever.
- [ ] Write onboarding doc
",
    )
    .unwrap();
    path
}

#[test]
fn list_shows_active_items_with_states() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed)
        .arg("list")
        .assert()
        .success()
        .stdout(contains("[ ] Fix auth redirect loop"))
        .stdout(contains("[ ] Write onboarding doc"));
}

#[test]
fn show_prints_title_and_body() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed)
        .args(["show", "auth"])
        .assert()
        .success()
        .stdout(contains("Fix auth redirect loop"))
        .stdout(contains("Repro: bounces forever."));
}

#[test]
fn claim_then_review_then_human_done() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed)
        .args(["claim", "auth", "--agent", "claude:abc123"])
        .assert()
        .success();
    assert!(std::fs::read_to_string(&feed)
        .unwrap()
        .contains("- [~] Fix auth redirect loop @agent(claude:abc123)"));

    feedr(&feed).args(["review", "auth"]).assert().success();
    assert!(std::fs::read_to_string(&feed)
        .unwrap()
        .contains("- [?] Fix auth"));

    // Agent may not close a human item:
    feedr(&feed)
        .args(["done", "auth"])
        .assert()
        .failure()
        .stderr(contains("only the human"));
    // Human may:
    feedr(&feed)
        .args(["done", "auth", "--as-human"])
        .assert()
        .success();
    assert!(std::fs::read_to_string(&feed)
        .unwrap()
        .contains("- [x] Fix auth"));
}

#[test]
fn add_and_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed)
        .args([
            "add",
            "--agent-owned",
            "Investigate flaky test",
            "--body",
            "Seen in CI run 42.",
        ])
        .assert()
        .success();
    let text = std::fs::read_to_string(&feed).unwrap();
    assert!(text.contains("## Agent"));
    assert!(text.contains("- [ ] Investigate flaky test\n  Seen in CI run 42."));

    feedr(&feed)
        .args(["done", "onboarding", "--as-human"])
        .assert()
        .success();
    feedr(&feed).arg("sweep").assert().success();
    let text = std::fs::read_to_string(&feed).unwrap();
    assert!(text.contains("# Done"));
    assert!(text.contains("- [x] Write onboarding doc @done("));
}

#[test]
fn claim_requires_agent_ref() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed).args(["claim", "auth"]).assert().failure();
}

#[test]
fn unreadable_feed_errors_instead_of_clobbering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feed.md");
    std::fs::write(&path, [0xFF, 0xFE, 0x00, 0x41]).unwrap(); // invalid UTF-8
    feedr(&path)
        .args(["add", "New item"])
        .assert()
        .failure()
        .stderr(contains("cannot read"));
    // Original bytes untouched:
    assert_eq!(std::fs::read(&path).unwrap(), vec![0xFF, 0xFE, 0x00, 0x41]);
}

#[test]
fn feedr_feed_env_selects_feed() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.env_remove("FEEDR_FEED");
    cmd.env("FEEDR_FEED", &feed);
    cmd.arg("list")
        .assert()
        .success()
        .stdout(contains("Fix auth redirect loop"));
}

#[test]
fn list_shows_sections_and_hides_archive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feed.md");
    std::fs::write(
        &path,
        "\
# Feed

- [ ] Top task

## Later

- [ ] Later task

# Done

## Feed

- [x] Archived task @done(2026-09-01)
",
    )
    .unwrap();
    feedr(&path)
        .arg("list")
        .assert()
        .success()
        .stdout(contains("Later:"))
        .stdout(contains("[ ] Later task"))
        .stdout(predicates::str::contains("Archived task").not());
}

#[test]
fn add_with_section_lands_in_named_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feed.md");
    std::fs::write(
        &path,
        "\
# Feed

- [ ] Top task

## Later

- [ ] Later task
",
    )
    .unwrap();
    feedr(&path)
        .args([
            "add",
            "--section",
            "Later",
            "New later task",
            "--body",
            "extra context",
        ])
        .assert()
        .success();
    let text = std::fs::read_to_string(&path).unwrap();
    // Must land inside "## Later", after the section heading — not in the
    // default (first/"# Feed") section.
    let later_idx = text.find("## Later").expect("section heading kept");
    let new_item_idx = text.find("New later task").expect("item was added");
    assert!(
        new_item_idx > later_idx,
        "new item must land inside ## Later, got:\n{text}"
    );
    assert!(text.contains("extra context"));
}

#[test]
fn add_with_missing_section_fails() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed)
        .args(["add", "--section", "Nonexistent", "New item"])
        .assert()
        .failure()
        .stderr(contains("no item matches"));
    // Nothing was written on failure.
    assert!(!std::fs::read_to_string(&feed).unwrap().contains("New item"));
}

#[test]
fn sidebar_subcommand_is_wired() {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.args(["sidebar", "--help"])
        .assert()
        .success()
        .stdout(contains("--dock"));
}

// --- Plan 3: `auto-dock-hook` (herdr-plugin.toml's `tab.created` event
// hook entry point) --------------------------------------------------------
//
// `config::default_config_dir()` is hardcoded to `dirs::config_dir()` (not
// injectable like `--file`/`FEEDR_FEED`), so these tests sandbox it by
// setting HOME for the child process to a fresh tempdir — on macOS
// `dirs::config_dir()` resolves under `$HOME/Library/Application Support`,
// so this never touches the real user config.

fn sandboxed_config_dir(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library/Application Support/herdr-feedr")
}

#[test]
fn auto_dock_hook_is_noop_when_auto_dock_disabled() {
    let home = tempfile::tempdir().unwrap();
    // No config.toml at all -> auto_dock defaults to false (config.rs).
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.env("HOME", home.path())
        .args(["auto-dock-hook", "--tab-id", "w1:t9"])
        .assert()
        .success()
        .stdout(""); // silent no-op — never even shells out to herdr
}

#[test]
fn auto_dock_hook_requires_a_tab_id_when_enabled() {
    let home = tempfile::tempdir().unwrap();
    let cfg_dir = sandboxed_config_dir(home.path());
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.toml"), "[sidebar]\nauto_dock = true\n").unwrap();

    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.env("HOME", home.path())
        .env_remove("HERDR_TAB_ID")
        .arg("auto-dock-hook")
        .assert()
        .failure()
        .stderr(contains("no tab id"));
}

/// End-to-end through the real binary (config read -> dock::auto_dock_for_tab
/// -> herdr shell-out), with a fake `herdr` standing in via HERDR_BIN_PATH so
/// no live herdr instance is touched.
#[test]
fn auto_dock_hook_docks_into_the_named_tab_when_enabled() {
    let home = tempfile::tempdir().unwrap();
    let cfg_dir = sandboxed_config_dir(home.path());
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.toml"), "[sidebar]\nauto_dock = true\n").unwrap();

    let log = home.path().join("calls.log");
    let fake_herdr = home.path().join("fake-herdr.sh");
    std::fs::write(
        &fake_herdr,
        format!(
            r#"#!/bin/sh
echo "$@" >> "{log}"
case "$1 $2" in
  "pane list") echo '{{"result":{{"panes":[{{"pane_id":"w1:p1","tab_id":"w1:t9","terminal_title_stripped":"vim"}}]}}}}' ;;
  "pane split") echo '{{"result":{{"pane_id":"w1:p10"}}}}' ;;
  *) echo '{{}}' ;;
esac
"#,
            log = log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake_herdr, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.env("HOME", home.path())
        .env("HERDR_BIN_PATH", &fake_herdr)
        .args(["auto-dock-hook", "--tab-id", "w1:t9"])
        .assert()
        .success()
        .stdout(contains("auto-docked"));

    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("pane list"), "got:\n{calls}");
    assert!(
        calls.contains("pane split --pane w1:p1 --direction right --no-focus"),
        "must split beside the target tab's own pane, no-focus; got:\n{calls}"
    );
    assert!(
        calls.contains("pane send-text w1:p10 exec feedr sidebar"),
        "got:\n{calls}"
    );
}

#[test]
fn auto_dock_hook_subcommand_is_wired() {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.args(["auto-dock-hook", "--help"])
        .assert()
        .success()
        .stdout(contains("--tab-id"));
}
