//! scripts/check-release-version.sh, the release workflow's gate: a tag must
//! name the version both manifests declare, or nothing is built or published.

use assert_cmd::Command;
use predicates::str::contains;
use std::path::Path;

fn check_release_version(root: &Path, tag: &str) -> assert_cmd::assert::Assert {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/check-release-version.sh");
    Command::new("/bin/sh")
        .arg(script)
        .arg(tag)
        .env("FEEDR_REPO_ROOT", root)
        .assert()
}

fn manifests(cargo: &str, plugin: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!("[package]\nname = \"feedr\"\nversion = \"{cargo}\"\n\n[dependencies]\nfoo = {{ version = \"1\" }}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("herdr-plugin.toml"),
        format!("id = \"herdr-feedr\"\nversion = \"{plugin}\"\nmin_herdr_version = \"0.9.0\"\n"),
    )
    .unwrap();
    dir
}

#[test]
fn release_accepts_a_tag_matching_both_manifests() {
    let dir = manifests("1.2.3", "1.2.3");
    check_release_version(dir.path(), "v1.2.3").success();
}

#[test]
fn release_refuses_a_tag_that_disagrees_with_cargo_toml() {
    let dir = manifests("1.2.3", "1.2.4");
    check_release_version(dir.path(), "v1.2.4")
        .failure()
        .stderr(contains("Cargo.toml version (1.2.3)"));
}

#[test]
fn release_refuses_a_tag_that_disagrees_with_the_plugin_manifest() {
    let dir = manifests("1.2.3", "1.2.2");
    check_release_version(dir.path(), "v1.2.3")
        .failure()
        .stderr(contains("herdr-plugin.toml version (1.2.2)"));
}

#[test]
fn the_real_manifests_agree_with_each_other() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    check_release_version(root, &tag).success();
}
