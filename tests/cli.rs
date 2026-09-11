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
fn sidebar_subcommand_is_wired() {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.args(["sidebar", "--help"])
        .assert()
        .success()
        .stdout(contains("--dock"));
}
