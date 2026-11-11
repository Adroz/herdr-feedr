#!/usr/bin/env bash
# herdr-plugin.toml's `[[startup]]` hook: bring the sidebars back after a herdr restart.
#
# herdr's session file records a plugin pane as only its cwd and label, not its command,
# so a restored session reopens every sidebar pane as a bare shell in the plugin root.
# herdr runs this once the session is restored and its socket is ready (and again when a
# new server takes over in a live handoff); `feedr revive-sidebars` execs the sidebar
# back into each of those shells, in place. See src/tui/dock.rs's `revive_sidebars`.
#
# No `--fit`: no client may be attached yet, so the tab width herdr reports can be its
# headless default rather than the real window's. `--wait` covers shells that are still
# reading their rc files when the hook fires.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
feedr_bin="${FEEDR_BIN_PATH:-$script_dir/../target/release/feedr}"

exec "$feedr_bin" revive-sidebars --wait 20
