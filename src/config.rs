use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SidebarConfig {
    pub side: Side,
    /// Relative resize delta used when docking/collapsing (herdr resize
    /// amounts are proportional shares, not columns; 0.18 matches herdr-beads).
    pub width: f64,
    /// Read for Plan 3's tab.created auto-dock hook; the TUI itself ignores it.
    pub auto_dock: bool,
}

#[derive(serde::Deserialize, Default)]
struct SidebarToml {
    side: Option<String>,
    width: Option<f64>,
    auto_dock: Option<bool>,
}

#[derive(Deserialize, Default)]
struct FileConfig {
    feed_path: Option<PathBuf>,
    #[serde(default)]
    sidebar: SidebarToml,
}

pub fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("herdr-feedr")
}

fn read_file_config(config_dir: &std::path::Path) -> FileConfig {
    match std::fs::read_to_string(config_dir.join("config.toml")) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("feedr: warning: ignoring malformed config.toml: {e}");
            FileConfig::default()
        }),
        Err(_) => FileConfig::default(),
    }
}

pub fn load_sidebar_config(config_dir: &std::path::Path) -> SidebarConfig {
    let s = read_file_config(config_dir).sidebar;
    let side = match s.side.as_deref() {
        None | Some("left") => Side::Left,
        Some("right") => Side::Right,
        Some(other) => {
            eprintln!("feedr: warning: sidebar.side \"{other}\" is not left/right; using left");
            Side::Left
        }
    };
    SidebarConfig {
        side,
        width: s.width.unwrap_or(0.18),
        auto_dock: s.auto_dock.unwrap_or(false),
    }
}

pub fn resolve_feed_path(
    cli_file: Option<PathBuf>,
    env_file: Option<PathBuf>,
    config_dir: &std::path::Path,
) -> PathBuf {
    if let Some(p) = cli_file {
        return p;
    }
    if let Some(p) = env_file {
        return p;
    }
    read_file_config(config_dir)
        .feed_path
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| config_dir.join("feed.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_cli_env_config_default() {
        let dir = tempfile::tempdir().unwrap();
        // Default when nothing set:
        assert_eq!(
            resolve_feed_path(None, None, dir.path()),
            dir.path().join("feed.md")
        );
        // Config file wins over default:
        std::fs::write(
            dir.path().join("config.toml"),
            "feed_path = \"/tmp/custom.md\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve_feed_path(None, None, dir.path()),
            PathBuf::from("/tmp/custom.md")
        );
        // Env wins over config:
        assert_eq!(
            resolve_feed_path(None, Some("/tmp/env.md".into()), dir.path()),
            PathBuf::from("/tmp/env.md")
        );
        // CLI wins over all:
        assert_eq!(
            resolve_feed_path(
                Some("/tmp/cli.md".into()),
                Some("/tmp/env.md".into()),
                dir.path()
            ),
            PathBuf::from("/tmp/cli.md")
        );
        // Empty feed_path treated as unset:
        std::fs::write(dir.path().join("config.toml"), "feed_path = \"\"\n").unwrap();
        assert_eq!(
            resolve_feed_path(None, None, dir.path()),
            dir.path().join("feed.md")
        );
    }

    #[test]
    fn sidebar_config_defaults_and_parse() {
        let dir = tempfile::tempdir().unwrap();
        // Defaults when no config file exists:
        assert_eq!(
            load_sidebar_config(dir.path()),
            SidebarConfig {
                side: Side::Left,
                width: 0.18,
                auto_dock: false
            }
        );
        // Parsed values:
        std::fs::write(
            dir.path().join("config.toml"),
            "feed_path = \"/tmp/f.md\"\n\n[sidebar]\nside = \"right\"\nwidth = 0.25\nauto_dock = true\n",
        )
        .unwrap();
        assert_eq!(
            load_sidebar_config(dir.path()),
            SidebarConfig {
                side: Side::Right,
                width: 0.25,
                auto_dock: true
            }
        );
        // Invalid side falls back to left:
        std::fs::write(
            dir.path().join("config.toml"),
            "[sidebar]\nside = \"top\"\n",
        )
        .unwrap();
        assert_eq!(load_sidebar_config(dir.path()).side, Side::Left);
        // feed_path resolution still works with [sidebar] present:
        std::fs::write(
            dir.path().join("config.toml"),
            "feed_path = \"/tmp/f.md\"\n[sidebar]\nwidth = 0.2\n",
        )
        .unwrap();
        assert_eq!(
            resolve_feed_path(None, None, dir.path()),
            PathBuf::from("/tmp/f.md")
        );
    }
}
