//! Skill ↔ CLI consistency (SPEC §5).
//!
//! herdr-file-viewer shipped a skill that told agents to pass a flag its
//! launcher never passed (#139). The skill is the only place an agent learns
//! this CLI's contract, so the two are tested against each other in both
//! directions: nothing documented may fail to parse, and nothing parseable may
//! go undocumented.

use assert_cmd::Command;

const SKILL: &str = include_str!("../skills/herdr-feedr/SKILL.md");

/// Split a documented command line the way a shell would: honour double quotes,
/// drop a trailing `# comment`.
fn shell_split(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => break,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
        let _ = chars.peek();
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Every `feedr ...` line the skill tells an agent to run.
fn documented_invocations() -> Vec<Vec<String>> {
    SKILL
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("feedr "))
        .map(shell_split)
        .map(|args| args[1..].to_vec())
        .collect()
}

/// Clap's own complaints about a command line it can't parse.
const PARSE_ERRORS: [&str; 5] = [
    "unrecognized subcommand",
    "unexpected argument",
    "invalid value",
    "required arguments were not provided",
    "unexpected value",
];

#[test]
fn every_command_the_skill_documents_parses() {
    let invocations = documented_invocations();
    assert!(
        invocations.len() >= 8,
        "expected the skill to document the CLI; found {} lines",
        invocations.len()
    );
    let dir = tempfile::tempdir().unwrap();
    let feed = dir.path().join("feed.md");
    std::fs::write(&feed, "# Feed\n\n- [ ] Fix auth redirect loop\n").unwrap();

    for args in invocations {
        let out = Command::cargo_bin("feedr")
            .unwrap()
            .arg("--file")
            .arg(&feed)
            .args(&args)
            // The command may legitimately fail (no identity to resolve, say);
            // only a *parse* failure means the skill and the CLI disagree.
            .env("HOME", dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        for complaint in PARSE_ERRORS {
            assert!(
                !stderr.contains(complaint),
                "skill documents `feedr {}`, which the CLI rejects: {stderr}",
                args.join(" ")
            );
        }
    }
}

#[test]
fn every_subcommand_is_documented_in_the_skill() {
    let help = Command::cargo_bin("feedr")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    let commands: Vec<String> = help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  ") && !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .filter(|c| c != "help")
        .collect();
    assert!(!commands.is_empty(), "could not read the command list");

    for command in commands {
        assert!(
            SKILL.contains(&format!("feedr {command}")),
            "`feedr {command}` exists but the skill never mentions it — an agent \
             can't be expected to know about a command that isn't written down"
        );
    }
}

#[test]
fn the_skill_stamp_matches_the_crate_and_the_plugin_manifest() {
    let crate_version = env!("CARGO_PKG_VERSION");

    let stamped = SKILL
        .split_once("Skill version ")
        .map(|(_, rest)| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>()
        })
        .expect("the skill must carry a version stamp");
    assert_eq!(stamped, crate_version, "skill stamp vs Cargo.toml");

    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"))
            .expect("herdr-plugin.toml");
    let manifest_version = manifest
        .lines()
        .find_map(|l| l.strip_prefix("version = "))
        .map(|v| v.trim().trim_matches('"').to_string())
        .expect("herdr-plugin.toml must declare a version");
    assert_eq!(
        manifest_version, crate_version,
        "herdr-plugin.toml vs Cargo.toml"
    );
}
