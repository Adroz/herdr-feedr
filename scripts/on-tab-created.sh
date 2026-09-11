#!/usr/bin/env bash
# herdr-plugin.toml's `[[events]]` `tab.created` hook (Plan 3): when feedr's own
# resolved config has `[sidebar] auto_dock = true`, dock a feedr-sidebar pane into the
# JUST-CREATED tab. Idempotent per tab: a sidebar pane already open in THAT tab is a
# no-op (see dock.rs's `auto_dock_for_tab`); a sidebar pane open in some OTHER tab does
# NOT count — this is deliberately scoped per-tab, unlike the `open-feedr` action's
# workspace-wide idempotency check.
#
# Mechanism (verified against a live herdr 0.9.0 install, `herdr api schema --json` and
# herdr.dev/docs/plugins): manifest-level `[[events]]` hooks are real in 0.9.0 --
# `InstalledPluginInfo.events` / `PluginManifestEventHook { on, command, platforms }` --
# and the docs' own worked example uses this exact shape (`on = "worktree.created"`).
# The installed herdr-file-viewer plugin's manifest comment ("there is no [[events]]
# hook (AC-N4)") documents a choice not to use this mechanism for that plugin, not that
# it doesn't exist -- confirmed live via `herdr plugin list --json`'s
# `events`/`startup` fields on `InstalledPluginInfo`.
#
# Context: herdr sets HERDR_TAB_ID directly (docs, "Commands and environment": direct
# env vars for common ids "when available") for an event hook tied to a tab, alongside
# the fuller HERDR_PLUGIN_EVENT_JSON payload. We prefer the direct var; if a future herdr
# version stops setting it for this event, fall back to pulling "tab_id" out of the event
# JSON (same string-scrape style as open-feedr.sh/toggle-feedr.sh use for `pane list`, to
# avoid a jq/python dependency).
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
feedr_bin="${FEEDR_BIN_PATH:-$script_dir/../target/release/feedr}"

tab_id="${HERDR_TAB_ID:-}"
if [ -z "$tab_id" ] && [ -n "${HERDR_PLUGIN_EVENT_JSON:-}" ]; then
  tab_id="$(printf '%s' "$HERDR_PLUGIN_EVENT_JSON" | sed -n 's/.*"tab_id":"\([^"]*\)".*/\1/p' | head -n1)"
fi

if [ -z "$tab_id" ]; then
  echo "herdr-feedr: tab.created hook fired without a tab id (checked HERDR_TAB_ID and HERDR_PLUGIN_EVENT_JSON); skipping auto-dock" >&2
  exit 0
fi

exec "$feedr_bin" auto-dock-hook --tab-id "$tab_id"
