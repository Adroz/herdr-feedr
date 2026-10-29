//! scripts/fetch-or-build.sh, the plugin's `[[build]]` step, run against a
//! fake GitHub release served from disk (`file://` URLs, through the real
//! `curl`), with `uname` and `cargo` stubbed on PATH.

use assert_cmd::Command;
use predicates::str::contains;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const VERSION: &str = "9.8.7";

/// A sandboxed plugin checkout, release server and PATH stubs.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    releases: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugin");
        let releases = dir.path().join("releases");
        let bin = dir.path().join("bin");
        let log = dir.path().join("calls.log");
        for d in [&root, &releases, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&log, "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"feedr\"\nversion = \"{VERSION}\"\n"),
        )
        .unwrap();
        // `cargo build --release` stands in for a source build: it records
        // the call and leaves a `feedr` that records its own calls.
        write_exe(
            &bin.join("cargo"),
            &format!(
                "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" >> '{log}'\n\
                 mkdir -p target/release\n\
                 printf '#!/bin/sh\\nprintf \"source-feedr %%s\\\\n\" \"$*\" >> {log}\\n' > target/release/feedr\n\
                 chmod +x target/release/feedr\n",
                log = log.display()
            ),
        );
        Fixture {
            _dir: dir,
            root,
            releases,
            bin,
            log,
        }
    }

    /// Make `uname -s` / `uname -m` report this platform.
    fn platform(self, os: &str, arch: &str) -> Self {
        write_exe(
            &self.bin.join("uname"),
            &format!("#!/bin/sh\ncase \"$1\" in -s) echo {os} ;; -m) echo {arch} ;; esac\n"),
        );
        self
    }

    /// Publish `herdr-feedr-<triple>` for [`VERSION`] — a `feedr` that
    /// records its calls as `prebuilt-feedr ...` — plus a `SHA256SUMS`
    /// listing it (with its true digest unless `digest` overrides it).
    fn publish(self, triple: &str, digest: Option<&str>) -> Self {
        let dir = self.releases.join(format!("v{VERSION}"));
        std::fs::create_dir_all(&dir).unwrap();
        let asset = format!("herdr-feedr-{triple}");
        std::fs::write(
            dir.join(&asset),
            format!(
                "#!/bin/sh\nprintf 'prebuilt-feedr %s\\n' \"$*\" >> '{}'\n",
                self.log.display()
            ),
        )
        .unwrap();
        let digest = digest.map_or_else(|| sha256(&dir.join(&asset)), str::to_string);
        std::fs::write(dir.join("SHA256SUMS"), format!("{digest}  {asset}\n")).unwrap();
        self
    }

    fn run(&self) -> assert_cmd::assert::Assert {
        self.command().assert()
    }

    fn command(&self) -> Command {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/fetch-or-build.sh");
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(script)
            .env("HOME", self.root.join("home"))
            .env("PATH", path)
            .env("FEEDR_REPO_ROOT", &self.root)
            .env_remove("FEEDR_FROM_SOURCE")
            .env(
                "FEEDR_RELEASE_BASE_URL",
                format!("file://{}", self.releases.display()),
            );
        cmd
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn sha256(file: &Path) -> String {
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("sha256sum \"$1\" 2>/dev/null || shasum -a 256 \"$1\"")
        .arg("sh")
        .arg(file)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()[..64].to_string()
}

#[test]
fn installs_the_verified_prebuilt_for_this_platform_and_refreshes_the_skill() {
    let fx = Fixture::new()
        .platform("Darwin", "arm64")
        .publish("aarch64-apple-darwin", None);

    fx.run()
        .success()
        .stdout(contains("installed prebuilt v9.8.7 (aarch64-apple-darwin)"));

    assert_eq!(
        fx.calls(),
        ["prebuilt-feedr skill install --refresh-only"],
        "no cargo build"
    );
    let mode = std::fs::metadata(fx.root.join("target/release/feedr"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "installed binary is executable");
}

#[test]
fn a_checksum_mismatch_falls_back_to_a_source_build() {
    let fx = Fixture::new()
        .platform("Linux", "x86_64")
        .publish("x86_64-unknown-linux-musl", Some(&"0".repeat(64)));

    fx.run().success().stderr(contains(
        "checksum mismatch for herdr-feedr-x86_64-unknown-linux-musl",
    ));

    assert_eq!(
        fx.calls(),
        [
            "cargo build --release",
            "source-feedr skill install --refresh-only"
        ]
    );
}

#[test]
fn an_unmapped_platform_falls_back_to_a_source_build() {
    let fx = Fixture::new()
        .platform("Linux", "aarch64")
        .publish("x86_64-unknown-linux-musl", None);

    fx.run()
        .success()
        .stderr(contains("no prebuilt binary for Linux/aarch64"));

    assert_eq!(
        fx.calls(),
        [
            "cargo build --release",
            "source-feedr skill install --refresh-only"
        ]
    );
}

#[test]
fn a_version_with_no_release_falls_back_to_a_source_build() {
    let fx = Fixture::new().platform("Darwin", "x86_64");

    fx.run().success().stderr(contains(
        "no prebuilt herdr-feedr-x86_64-apple-darwin for v9.8.7",
    ));

    assert_eq!(
        fx.calls(),
        [
            "cargo build --release",
            "source-feedr skill install --refresh-only"
        ]
    );
}

/// Local development: build the checkout even when a release exists.
#[test]
fn feedr_from_source_skips_the_prebuilt() {
    let fx = Fixture::new()
        .platform("Darwin", "arm64")
        .publish("aarch64-apple-darwin", None);

    fx.command()
        .env("FEEDR_FROM_SOURCE", "1")
        .assert()
        .success();

    assert_eq!(
        fx.calls(),
        [
            "cargo build --release",
            "source-feedr skill install --refresh-only"
        ]
    );
}
