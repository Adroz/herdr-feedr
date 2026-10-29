#!/usr/bin/env bash
# The `open-feedr` action (bind it to a keybinding via a herdr `[[keys.command]]` of type
# "shell"): open-or-focus the feedr sidebar. A thin wrapper over `feedr sidebar --dock`
# (src/tui/dock.rs's `dock()`), which owns all the docking logic: it finds a sidebar
# already open in any workspace and focuses it, or opens one beside the focused pane
# and docks it on the configured `[sidebar]` side at the configured width/max_width.
# HERDR_BIN_PATH / HERDR_SOCKET_PATH, when set, pass through to feedr unchanged.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
feedr_bin="${FEEDR_BIN_PATH:-$script_dir/../target/release/feedr}"

exec "$feedr_bin" sidebar --dock
