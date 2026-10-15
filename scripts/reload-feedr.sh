#!/usr/bin/env bash
# Reload every open feedr sidebar onto a freshly built binary. herdr never hot-reloads a
# running pane's process, so after `cargo build --release` each sidebar keeps running the
# old code until it is restarted. This closes each feedr-sidebar pane and immediately
# re-docks a new one into the same tab (`feedr auto-dock-hook --force`, the same per-tab
# dock the tab.created hook uses: left edge, configured width, no focus steal).
#
# Usage: scripts/reload-feedr.sh [--no-build] [--dry-run] [--tab <tab_id>]...
#   --no-build   skip `cargo build --release` (binary already fresh)
#   --dry-run    print what would be reloaded; touch nothing
#   --tab ID     only reload the sidebar in this tab (repeatable); default is every tab
#                in every workspace
#
# A failed build aborts before any pane is touched. A sidebar that is the only pane in
# its tab is skipped: closing it would close the tab, and there'd be nothing to re-dock
# beside. Per-pane state that lives only in the process (collapsed, scroll) resets.
#
# herdr activates a tab whenever a pane in it is swapped, so re-docking into background
# tabs would drag the user's view around. Each workspace's active tab is snapshotted up
# front and restored at the end, the focused workspace last so it stays in front.
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
  panes_in_tab="$(printf '%s\n' "$objects" | grep -c "\"tab_id\":\"${tab_id}\"" || true)"
  if [ "$panes_in_tab" -le 1 ]; then
    echo "skip   $tab_id: sidebar $pane_id is the tab's only pane"
    skipped=$((skipped + 1))
    continue
  fi
  if [ "$dry_run" -eq 1 ]; then
    echo "would  $tab_id: reload sidebar $pane_id"
    continue
  fi
  if ! "$herdr_bin" pane close "$pane_id" >/dev/null; then
    echo "FAIL   $tab_id: could not close $pane_id" >&2
    failed=$((failed + 1))
    continue
  fi
  if msg="$("$feedr_bin" auto-dock-hook --force --tab-id "$tab_id" 2>&1)"; then
    echo "ok     $tab_id: $msg"
    reloaded=$((reloaded + 1))
  else
    echo "FAIL   $tab_id: closed $pane_id but re-dock failed: $msg" >&2
    failed=$((failed + 1))
  fi
done <<<"$sidebars"

echo "reload-feedr: $reloaded reloaded, $skipped skipped, $failed failed"
[ "$failed" -eq 0 ]
