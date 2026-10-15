#!/usr/bin/env bash
# Reload every open feedr sidebar onto a freshly built binary. herdr never hot-reloads a
# running pane's process, so after `cargo build --release` each sidebar keeps running the
# old code until it is restarted.
#
# Each sidebar is replaced IN PLACE: a new sidebar is split off the old one, then the old
# one closes and the new one inherits its exact slot in the tab's split tree — no other
# pane moves or resizes. `feedr fit-sidebar` then sets it to the configured width exactly
# (its `width` share of the tab, capped at `max_width`), taking or giving the columns on
# the far side of the tab's top split.
#
# Usage: scripts/reload-feedr.sh [--no-build] [--dry-run] [--tab <tab_id>]...
#   --no-build   skip `cargo build --release` (binary already fresh)
#   --dry-run    print what would be reloaded; touch nothing
#   --tab ID     only reload the sidebar in this tab (repeatable); default is every tab
#                in every workspace
#
# A failed build aborts before any pane is touched. If the in-place split fails, it falls
# back to closing the sidebar and re-docking a fresh one (`feedr auto-dock-hook --force`),
# skipping a sidebar that is its tab's only pane since closing it would close the tab.
# Per-pane state that lives only in the process (collapsed, scroll) resets.
#
# Some herdr layout ops activate the tab they touch, which would drag the user's view
# around. Each workspace's active tab is snapshotted up front and restored at the end,
# the focused workspace last so it stays in front.
#
# Pane discovery matches the terminal title the sidebar sets on startup — same approach
# and same sed-over-sorted-JSON parsing as open-feedr.sh (see its header for why).
set -euo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
title_marker="feedr-sidebar" # must match src/tui/dock.rs's PANE_TITLE_MARKER
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
repo_root="$script_dir/.."
feedr_bin="${FEEDR_BIN_PATH:-$repo_root/target/release/feedr}"

build=1
dry_run=0
only_tabs=()
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) build=0 ;;
    --dry-run) dry_run=1 ;;
    --tab)
      [ $# -ge 2 ] || { echo "reload-feedr: --tab needs a tab id" >&2; exit 2; }
      only_tabs+=("$2")
      shift
      ;;
    *) echo "reload-feedr: unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ "$build" -eq 1 ] && [ "$dry_run" -eq 0 ]; then
  (cd "$repo_root" && cargo build --release)
fi

# One JSON pane object per line. Keys are alphabetically sorted in `pane list` output,
# so pane_id < tab_id < terminal_title_stripped within an object.
pane_objects() {
  "$herdr_bin" pane list | sed 's/},{/}\n{/g'
}

wanted_tab() {
  [ "${#only_tabs[@]}" -eq 0 ] && return 0
  local t
  for t in "${only_tabs[@]}"; do [ "$t" = "$1" ] && return 0; done
  return 1
}

# "workspace_id active_tab_id focused" per workspace, the focused one last.
workspace_view() {
  "$herdr_bin" workspace list \
    | sed 's/},{/}\n{/g' \
    | sed -n 's/.*"active_tab_id":"\([^"]*\)".*"focused":\([a-z]*\).*"workspace_id":"\([^"]*\)".*/\3 \1 \2/p' \
    | sort -k3
}

restore_view() {
  local ws tab focused
  while read -r ws tab focused; do
    [ -n "$tab" ] && "$herdr_bin" tab focus "$tab" >/dev/null 2>&1 || true
  done <<<"$view"
}

objects="$(pane_objects)"
view="$(workspace_view)"
# "pane_id tab_id" of every sidebar pane, across all workspaces.
sidebars="$(printf '%s\n' "$objects" \
  | grep "\"terminal_title_stripped\":\"${title_marker}\"" \
  | sed -n 's/.*"pane_id":"\([^"]*\)".*"tab_id":"\([^"]*\)".*/\1 \2/p' || true)"

if [ -z "$sidebars" ]; then
  echo "reload-feedr: no open feedr sidebars"
  exit 0
fi

reloaded=0
skipped=0
failed=0
[ "$dry_run" -eq 1 ] || trap restore_view EXIT
while read -r pane_id tab_id; do
  wanted_tab "$tab_id" || continue
  if [ "$dry_run" -eq 1 ]; then
    echo "would  $tab_id: reload sidebar $pane_id"
    continue
  fi

  # In place: split the new sidebar off the old, then close the old.
  new_id="$("$herdr_bin" plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
    --placement split --direction right --target-pane "$pane_id" --no-focus 2>/dev/null \
    | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' | head -n1 || true)"
  if [ -n "$new_id" ]; then
    if ! "$herdr_bin" pane close "$pane_id" >/dev/null; then
      echo "FAIL   $tab_id: opened $new_id but could not close old sidebar $pane_id" >&2
      failed=$((failed + 1))
      continue
    fi
    if fit="$("$feedr_bin" fit-sidebar --pane "$new_id" 2>&1)"; then
      echo "ok     $tab_id: replaced $pane_id with $new_id"
    else
      echo "ok     $tab_id: replaced $pane_id with $new_id (width not fitted: $fit)"
    fi
    reloaded=$((reloaded + 1))
    continue
  fi

  # Fallback: close and re-dock. Never close a tab's only pane.
  panes_in_tab="$(printf '%s\n' "$objects" | grep -c "\"tab_id\":\"${tab_id}\"" || true)"
  if [ "$panes_in_tab" -le 1 ]; then
    echo "skip   $tab_id: in-place reload failed and sidebar $pane_id is the tab's only pane"
    skipped=$((skipped + 1))
    continue
  fi
  if ! "$herdr_bin" pane close "$pane_id" >/dev/null; then
    echo "FAIL   $tab_id: could not close $pane_id" >&2
    failed=$((failed + 1))
    continue
  fi
  if msg="$("$feedr_bin" auto-dock-hook --force --tab-id "$tab_id" 2>&1)"; then
    echo "ok     $tab_id: $msg (re-docked)"
    reloaded=$((reloaded + 1))
  else
    echo "FAIL   $tab_id: closed $pane_id but re-dock failed: $msg" >&2
    failed=$((failed + 1))
  fi
done <<<"$sidebars"

echo "reload-feedr: $reloaded reloaded, $skipped skipped, $failed failed"
[ "$failed" -eq 0 ]
