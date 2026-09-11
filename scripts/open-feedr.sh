#!/usr/bin/env bash
# Idempotent launcher for the feedr-sidebar plugin pane — used by the `open-feedr` action
# (bind it to a keybinding via a herdr `[[keys.command]]` of type "shell").
#
# "Open-or-focus, docked left": a feedr-sidebar pane already open anywhere in this
# workspace -> focus it; otherwise open one as a split, swap-walk it to the left edge,
# and resize it narrow. This mirrors src/tui/dock.rs's `dock()` (the logic behind
# `feedr sidebar --dock`), but goes through herdr's PLUGIN pane commands
# (`plugin pane open`/`plugin pane focus`) instead of the raw `pane split` dock.rs uses:
# this launcher only runs once the plugin is installed, and herdr needs to track the pane
# as plugin-owned for `plugin pane close`, reinstall/uninstall bookkeeping, etc. to see it.
#
# Existing-pane detection: `herdr plugin pane --help` (0.9.0) offers open/focus/close by
# PANE_ID only — there is no "list panes owned by this plugin" query — and
# `herdr plugin list --json` describes installed plugin MANIFESTS (panes/actions/build),
# not which panes are currently open (verified read-only against a live install). So,
# like dock.rs, this falls back to `herdr pane list` and matches the exact terminal title
# the sidebar sets on startup (src/tui/mod.rs's `SetTitle(dock::PANE_TITLE_MARKER)`, which
# runs on every launch path, plugin or not — confirmed live: a `feedr sidebar` pane shows
# up in `pane list` with `"terminal_title_stripped":"feedr-sidebar"`).
#
# `herdr pane list` prints one JSON line with alphabetically-sorted object keys, so
# within each pane object "pane_id" always precedes "terminal_title_stripped" — a plain
# sed/grep pass (no jq/python dependency) can pull the match out reliably; verified
# read-only against a live `herdr pane list` during development.
set -euo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
title_marker="feedr-sidebar" # must match src/tui/dock.rs's PANE_TITLE_MARKER

# Dock amounts mirror src/tui/dock.rs's SidebarConfig default (side = left, width = 0.18
# — see src/config.rs's load_sidebar_config). TODO: honor a user's [sidebar] side/width
# from ~/.config/herdr-feedr/config.toml (Linux) or ~/Library/Application
# Support/herdr-feedr/config.toml (macOS's dirs::config_dir()), like dock.rs's `dock()`
# does. Deferred for v1 to avoid a bespoke TOML parser in bash; the plugin still docks
# correctly, just always to the left at the 0.18 default.
dock_side="left"
dock_amount="0.18"

# pane_id of an already-open feedr-sidebar pane, if any (empty string if none/on error).
find_sidebar_pane() {
  "$herdr_bin" pane list 2>/dev/null \
    | sed 's/},{/}\n{/g' \
    | grep "\"terminal_title_stripped\":\"${title_marker}\"" \
    | head -n1 \
    | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' || true
}

# First "pane_id" value in a `plugin pane open` JSON response.
extract_pane_id() {
  sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' | head -n1
}

existing_pane_id="$(find_sidebar_pane || true)"
if [ -n "$existing_pane_id" ]; then
  exec "$herdr_bin" plugin pane focus "$existing_pane_id"
fi

open_out="$("$herdr_bin" plugin pane open \
  --plugin herdr-feedr \
  --entrypoint feedr-sidebar \
  --placement split \
  --direction right \
  --focus)"
pane_id="$(printf '%s' "$open_out" | extract_pane_id)"
if [ -z "$pane_id" ]; then
  echo "herdr-feedr: could not read pane_id from 'plugin pane open' output: $open_out" >&2
  exit 1
fi

if [ "$dock_side" = "left" ]; then
  i=0
  while [ "$i" -lt 6 ]; do
    if ! "$herdr_bin" pane neighbor --direction left --pane "$pane_id" >/dev/null 2>&1; then
      break # at the left edge
    fi
    if ! "$herdr_bin" pane swap --direction left --pane "$pane_id" >/dev/null 2>&1; then
      break
    fi
    i=$((i + 1))
  done
fi

exec "$herdr_bin" pane resize --direction "$dock_side" --amount "$dock_amount" --pane "$pane_id"
