use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Default)]
struct FileConfig {
    feed_path: Option<PathBuf>,
}

pub fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("herdr-feedr")
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
    let cfg: FileConfig = std::fs::read_to_string(config_dir.join("config.toml"))
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    cfg.feed_path
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
}
