//! Shipping the agent skill (SPEC §5, [#12](https://github.com/Adroz/herdr-feedr/issues/12)).
//!
//! herdr has no skill mechanism, so a plugin's `skills/` directory is inert
//! convention — something has to copy it where agent CLIs look. The binary owns
//! that job: the skill text is embedded at build time, so `feedr skill install`
//! works from an installed binary with no checkout present.
//!
//! Two target paths, and no others: `~/.agents/skills/herdr-feedr/` (the
//! agent-neutral home Codex/OpenCode/Pi read) and `~/.claude/skills/herdr-feedr/`
//! (Claude Code), the latter preferring a symlink into the former so one refresh
//! updates both.
//!
//! `--refresh-only` — what the plugin build step runs — refreshes copies that
//! already exist and never creates one. A `herdr plugin install` must not plant
//! files in someone's home directory unasked.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The skill text, embedded so installation needs no repo checkout.
pub const SKILL: &str = include_str!("../skills/herdr-feedr/SKILL.md");

pub const SKILL_NAME: &str = "herdr-feedr";

/// What `install` did to one path — reported line by line, because writing into
/// someone's home should never be silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Wrote(PathBuf),
    Linked { link: PathBuf, target: PathBuf },
    LeftAlone(PathBuf),
    SkippedNotInstalled(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub path: PathBuf,
    /// The stamp read from the installed copy, if it carries one.
    pub installed_version: Option<String>,
    pub stale: bool,
}

/// `> Skill version 0.1.0 — requires feedr CLI >= 0.1.0.`
pub fn stamped_version(text: &str) -> Option<String> {
    let (_, rest) = text.split_once("Skill version ")?;
    let v: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

pub fn agents_skill_dir(home: &Path) -> PathBuf {
    home.join(".agents").join("skills").join(SKILL_NAME)
}

pub fn claude_skill_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("skills").join(SKILL_NAME)
}

pub fn install(home: &Path, refresh_only: bool) -> Result<Vec<Action>> {
    let mut actions = Vec::new();
    let agents = agents_skill_dir(home);
    let claude = claude_skill_dir(home);

    if agents.exists() || !refresh_only {
        actions.push(write_copy(&agents)?);
    } else {
        actions.push(Action::SkippedNotInstalled(agents.clone()));
    }

    match claude.symlink_metadata() {
        // Already pointing somewhere — assume at the agent-neutral copy, which
        // the write above just refreshed. Repointing someone's symlink is not
        // ours to do.
        Ok(m) if m.file_type().is_symlink() => actions.push(Action::LeftAlone(claude)),
        // A real directory: someone (or an older install) copied rather than
        // linked. Refresh the copy in place instead of swapping it for a link.
        Ok(_) => actions.push(write_copy(&claude)?),
        Err(_) if refresh_only => actions.push(Action::SkippedNotInstalled(claude)),
        Err(_) => {
            if let Some(parent) = claude.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            // Relative, so the link survives a moved home directory.
            let target = PathBuf::from("../../.agents/skills").join(SKILL_NAME);
            std::os::unix::fs::symlink(&target, &claude)
                .with_context(|| format!("linking {}", claude.display()))?;
            actions.push(Action::Linked {
                link: claude,
                target,
            });
        }
    }
    Ok(actions)
}

fn write_copy(dir: &Path) -> Result<Action> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, SKILL).with_context(|| format!("writing {}", file.display()))?;
    Ok(Action::Wrote(file))
}

pub fn status(home: &Path) -> Vec<StatusRow> {
    let current = env!("CARGO_PKG_VERSION");
    [agents_skill_dir(home), claude_skill_dir(home)]
        .into_iter()
        .filter_map(|dir| {
            // A symlinked Claude dir resolves to the same file as the
            // agent-neutral copy; listing it twice would be noise.
            if dir.symlink_metadata().ok()?.file_type().is_symlink() {
                return None;
            }
            let file = dir.join("SKILL.md");
            let text = std::fs::read_to_string(&file).ok()?;
            let installed_version = stamped_version(&text);
            Some(StatusRow {
                path: file,
                stale: installed_version.as_deref() != Some(current),
                installed_version,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn a_stamp_is_read_out_of_surrounding_prose() {
        assert_eq!(
            stamped_version("> Skill version 1.2.3 — requires feedr CLI >= 1.2.3.").as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn an_unstamped_skill_reads_as_no_version() {
        assert_eq!(stamped_version("# herdr-feedr\n\nno stamp here"), None);
    }

    #[test]
    fn a_stamp_with_no_version_after_it_reads_as_no_version() {
        assert_eq!(stamped_version("> Skill version — oops"), None);
    }

    #[test]
    fn the_embedded_skill_carries_the_crate_version_stamp() {
        assert_eq!(
            stamped_version(SKILL).as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn install_writes_the_agent_neutral_copy() {
        let home = tempfile::tempdir().unwrap();

        let actions = install(home.path(), false).unwrap();

        let written = agents_skill_dir(home.path()).join("SKILL.md");
        assert_eq!(read(&written), SKILL);
        assert!(actions.contains(&Action::Wrote(written)));
    }

    #[test]
    fn install_symlinks_the_claude_dir_at_the_agent_neutral_copy() {
        let home = tempfile::tempdir().unwrap();

        install(home.path(), false).unwrap();

        let link = claude_skill_dir(home.path());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        // Resolves to the same text, so one refresh updates both.
        assert_eq!(read(&link.join("SKILL.md")), SKILL);
    }

    #[test]
    fn install_overwrites_a_real_directory_copy_rather_than_replacing_it_with_a_link() {
        let home = tempfile::tempdir().unwrap();
        let claude = claude_skill_dir(home.path());
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("SKILL.md"), "stale copy").unwrap();

        let actions = install(home.path(), false).unwrap();

        assert!(!claude.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(read(&claude.join("SKILL.md")), SKILL);
        assert!(actions.contains(&Action::Wrote(claude.join("SKILL.md"))));
    }

    #[test]
    fn install_leaves_an_existing_symlink_in_place() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), false).unwrap();

        let actions = install(home.path(), false).unwrap();

        let link = claude_skill_dir(home.path());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(actions.contains(&Action::LeftAlone(link)));
    }

    #[test]
    fn refresh_only_creates_nothing_when_the_skill_is_not_installed() {
        let home = tempfile::tempdir().unwrap();

        let actions = install(home.path(), true).unwrap();

        assert!(!agents_skill_dir(home.path()).exists());
        assert!(!claude_skill_dir(home.path()).exists());
        assert!(actions
            .iter()
            .all(|a| matches!(a, Action::SkippedNotInstalled(_))));
    }

    #[test]
    fn refresh_only_updates_a_copy_that_is_already_installed() {
        let home = tempfile::tempdir().unwrap();
        let agents = agents_skill_dir(home.path());
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("SKILL.md"), "> Skill version 0.0.1 — old").unwrap();

        install(home.path(), true).unwrap();

        assert_eq!(read(&agents.join("SKILL.md")), SKILL);
        // Still never creates the one that wasn't there.
        assert!(!claude_skill_dir(home.path()).exists());
    }

    #[test]
    fn status_flags_a_stale_installed_copy() {
        let home = tempfile::tempdir().unwrap();
        let agents = agents_skill_dir(home.path());
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("SKILL.md"), "> Skill version 0.0.1 — old").unwrap();

        let rows = status(home.path());

        let row = rows
            .iter()
            .find(|r| r.path.starts_with(&agents))
            .expect("the installed copy should be listed");
        assert_eq!(row.installed_version.as_deref(), Some("0.0.1"));
        assert!(row.stale);
    }

    #[test]
    fn status_reports_a_current_copy_as_fresh() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), false).unwrap();

        let rows = status(home.path());

        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| !r.stale), "{rows:?}");
    }

    #[test]
    fn status_lists_nothing_when_no_copy_is_installed() {
        let home = tempfile::tempdir().unwrap();

        assert!(status(home.path()).is_empty());
    }
}
