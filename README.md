# herdr-feedr

The feed rack for your herd — a markdown-backed to-do sidebar for [Herdr](https://herdr.dev), shared between you and your coding agents. You keep a plain to-do list; agents pull work from it, add their own, and report back — all through one file you can always open in any editor.

**Status: core + CLI implemented** (`cargo install --path .` → `feedr --help`). Sidebar TUI and herdr plugin packaging are next; the v1 spec is in [docs/SPEC.md](docs/SPEC.md); it was decided through the wayfinder map in [issue #1](https://github.com/Adroz/herdr-feedr/issues/1) (research findings live on `research/*` branches, format prototypes on `prototype/feed-format`).

Planned shape: one Rust binary — a ratatui sidebar pane (via the herdr plugin system) plus a `feedr` CLI that agents drive through a shipped skill.
