# herdr-feedr

The feed rack for your herd — a markdown-backed to-do sidebar for [Herdr](https://herdr.dev), shared between you and your coding agents. You keep a plain to-do list; agents pull work from it, add their own, and report back — all through one file you can always open in any editor.

**Status: core + CLI + sidebar TUI + herdr plugin packaging implemented** (`cargo install --path .` → `feedr sidebar`, or `feedr sidebar --dock` inside herdr; or install as a herdr plugin — see below). The agent skill is next; the v1 spec is in [docs/SPEC.md](docs/SPEC.md); it was decided through the wayfinder map in [issue #1](https://github.com/Adroz/herdr-feedr/issues/1) (research findings live on `research/*` branches, format prototypes on `prototype/feed-format`).

Planned shape: one Rust binary — a ratatui sidebar pane (via the herdr plugin system) plus a `feedr` CLI that agents drive through a shipped skill.

## Install as a herdr plugin

herdr-feedr ships a `herdr-plugin.toml` manifest (see [docs/SPEC.md §8](docs/SPEC.md)) that packages the sidebar as a herdr plugin pane, plus two actions to open it: `open-feedr` (open-or-focus, docked to the left edge) and `toggle-feedr` (close if open, else open). v1 supports Linux and macOS; there's no Windows packaging yet.

### Published install

```sh
herdr plugin install Adroz/herdr-feedr
```

herdr clones the repo, runs `scripts/build.sh` (`cargo build --release` — a Rust toolchain is required; there's no prebuilt-binary download in v1), and registers the `feedr-sidebar` pane and the `open-feedr`/`toggle-feedr` actions.

### Local development

Link this working tree instead of a published release:

```sh
herdr plugin link /path/to/herdr-feedr-sidebar
```

Rebuild loop while iterating:

1. Edit the code.
2. `cargo build --release` (or re-run `herdr plugin link <path>`, which re-runs `scripts/build.sh`).
3. Close and reopen the sidebar pane (`toggle-feedr` twice, or `plugin pane close` + `open-feedr`) to pick up the new binary — herdr doesn't hot-reload a running pane's process.

`herdr plugin list` shows whether herdr-feedr loaded, with its pane and both actions.

### Bind keys

Bind the actions to keys in `~/.config/herdr/config.toml` (`type = "plugin_action"`, `command = "<plugin-id>.<action-id>"` — the same shape herdr-file-viewer uses for its own binding):

```toml
[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "herdr-feedr.open-feedr"

[[keys.command]]
key = "prefix+shift+t"
type = "plugin_action"
command = "herdr-feedr.toggle-feedr"
```

(Pick keys that don't collide with existing bindings — check `~/.config/herdr/config.toml` for what's already taken.)
