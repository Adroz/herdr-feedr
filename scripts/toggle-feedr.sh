#!/usr/bin/env bash
# Idempotent open/close toggle for the feedr-sidebar plugin pane — used by the
# `toggle-feedr` action (bind it to a keybinding via a herdr `[[keys.command]]` of type
# "shell").
#
# "Toggle": a feedr-sidebar pane already open in any workspace -> close it; otherwise
# delegate to open-feedr.sh (`feedr sidebar --dock`, docked per `[sidebar]` config).
#
# Existing-pane detection matches the exact terminal title the running sidebar sets
# (src/tui/dock.rs's PANE_TITLE_MARKER) in `herdr pane list`: herdr has no `plugin pane
# list`, and `plugin list --json` only describes manifests, not currently-open panes.
# `pane list` prints one JSON line with alphabetically-sorted keys, so within each pane
# object "pane_id" precedes "terminal_title_stripped" and a sed/grep pass is enough.
set -euo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
title_marker="feedr-sidebar" # must match src/tui/dock.rs's PANE_TITLE_MARKER
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"

# pane_id of an already-open feedr-sidebar pane, if any (empty string if none/on error).
find_sidebar_pane() {
  "$herdr_bin" pane list 2>/dev/null \
    | sed 's/},{/}\n{/g' \
    | grep "\"terminal_title_stripped\":\"${title_marker}\"" \
    | head -n1 \
    | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' || true
}

existing_pane_id="$(find_sidebar_pane || true)"
if [ -n "$existing_pane_id" ]; then
  exec "$herdr_bin" plugin pane close "$existing_pane_id"
fi

exec "$script_dir/open-feedr.sh"
