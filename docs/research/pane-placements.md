# Research: pane placements for a persistent left sidebar (Herdr plugin v1)

**Ticket:** [#2](https://github.com/Adroz/herdr-feedr/issues/2) (part of #1)
**Herdr version investigated:** 0.9.0 (local install; herdr.dev llms.txt confirms 0.9.0 is current stable, so docs match the binary)

**Sources**

- Official docs: <https://herdr.dev/docs/plugins>, <https://herdr.dev/docs/socket-api>, <https://herdr.dev/docs/session-state>, <https://herdr.dev/docs/configuration>, <https://herdr.dev/llms.txt>
- Local CLI: `herdr plugin pane open --help`, `herdr pane {split,resize,swap,move,zoom} --help`, `herdr api schema --json` (protocol 22), plus live parse tests against the running 0.9.0 server
- [miiraheart/herdr-beads](https://github.com/miiraheart/herdr-beads) source (`herdr-plugin.toml`, `scripts/lib.sh`, `scripts/open-dock.sh`, `scripts/open-board.sh`, `scripts/auto-dock.sh`, README)
- herdr-file-viewer v1.16.0, installed at `~/.config/herdr/plugins/github/herdr-file-viewer-c993314e2614/` (`herdr-plugin.toml`, `scripts/open-file-viewer.sh`, `docs/summoning.md`, `docs/configuration.md`)

---

## TL;DR

A true "persistent left sidebar" is **not directly expressible** in plugin v1: splits only go `right`/`down`, panes are strictly per-tab, there is no fixed-width or hide-without-close primitive, and plugin panes are not restored as plugin panes after a server restart. The closest real thing is the **herdr-beads dock pattern**: open a `split`, swap-walk it to the left edge, shrink it with a resize delta, and (optionally) re-open it on every new tab via a `tab.created` event hook. That is what we should build on.

---

## The five placements (verified against 0.9.0)

`plugin.pane.open` / `herdr plugin pane open` accepts `--placement overlay|popup|split|tab|zoomed`. Manifest `[[panes]]` default is `overlay`.

> Note: the 0.9.0 CLI `--help` only lists `overlay, split, tab, zoomed` and omits `--width/--height` — but the CLI parser and socket API (schema `PluginPanePlacement`, `PluginPaneOpenParams`) accept `popup` plus `width`/`height`, verified by live parse test. herdr-beads relies on this. Treat the `--help` text as stale.

| Placement | Semantics (docs + observed) |
|---|---|
| `split` | A normal tiled Herdr pane split off a target pane. `--direction right\|down` **only** (no left/up). No ratio/width at open time via `plugin.pane.open` (plain `pane split` has `--ratio`, the plugin variant does not). After open it is an ordinary pane: addressable, resizable, swappable, movable. |
| `tab` | The plugin in its own tab in the current workspace. Ordinary pane thereafter. |
| `zoomed` | Opens as a normal pane but zoomed (full-tab). Ordinary pane thereafter. |
| `overlay` | "Temporary zoomed overlay over the active pane; restores the previous focus and zoom when it closes" (docs/plugins). Outside standard pane APIs; no `HERDR_PANE_ID`. |
| `popup` | "Session-modal terminal popup without changing the tiled layout." **A singleton session resource, not a pane**: no pane ID, invisible to `pane list`, no lifecycle events, no persistence, one per session (not per tab), grabs all keys while focused. Only placement with declarative sizing: `width`/`height` as cells or `"80%"` strings. |

Docs: "Split, tab, zoomed, and overlay plugin panes are normal Herdr panes after they open." — <https://herdr.dev/docs/plugins>

## Key facts for a sidebar

### Left docking

No API places a pane on the left. `SplitDirection` is `["right", "down"]` everywhere (CLI, socket schema, docs). **Workaround (herdr-beads `scripts/lib.sh`, `open_dock_at()`):** split right off the target pane, then swap-walk left:

```bash
for _ in 1 2 3 4 5 6; do
  herdr pane neighbor --direction left --pane "$dock" || break
  herdr pane swap --direction left --pane "$dock" || break
done
herdr pane resize --direction left --amount 0.18 --pane "$dock"
```

`pane swap` "preserves split shape, ratios, pane IDs, and running processes" (socket-api docs), so this reliably lands the pane at the left edge. Beads' manifest calls this "herdr-sidebar style: split then swap into the left slot."

### Persistence

- **Per-tab, not global.** A split pane lives in one tab's layout. Nothing offers a pinned pane spanning tabs. Both plugins scope their idempotent launchers accordingly: file-viewer's split toggle is per-tab, its tab placement per-workspace; beads' dock toggle is per-tab ("a workspace-wide match closes another tab's dock" — `lib.sh`).
- **"Every tab" is synthesized via events.** Beads' opt-in auto-dock: an `[[events]]` hook `on = "tab.created"` runs a script that opens a dock in each new tab (deduped), driven by a marker file in `$HERDR_PLUGIN_CONFIG_DIR` — so the preference survives restarts even though panes don't.
- **Across moves:** "Herdr keeps plugin pane ownership attached to the underlying pane when it moves across tabs or workspaces" (docs/plugins).
- **Across server restarts:** session-state restores layout, but "panes that cannot use a stronger restore path come back as new shells in their saved directories" — plugin panes are **not** documented to relaunch as the plugin. Assume the sidebar must re-summon itself (event hook or user action).

### Width control

- No fixed/minimum width for tiled panes anywhere. `plugin.pane.open` has no ratio; the split lands at the default share, then you shrink it with **relative** deltas: `pane resize --direction left|right|up|down --amount <float>` (proportional, not absolute — you must track your own "expanded" ratio). Beads shrinks its dock by `0.18` after docking.
- `--width`/`--height` on `plugin pane open` are `PopupSize` ("outer **popup** size in cells or `%`") — sizing for `popup` (beads' floating board uses `--width 85% --height 85%`), not a split-width mechanism.
- In-pane column widths (like file-viewer's `tree_width`/`tree_max_cols`) are the plugin's own business — "not the herdr pane, which the host decides" (file-viewer docs/configuration.md).

### What "collapse" maps to

There is no hide/minimize primitive ("herdr has no hide-without-close" — file-viewer launcher, still true in 0.9.0).

1. **Close/reopen (recommended).** Both plugins do launch-or-focus-or-toggle: pane absent → open; present unfocused → focus; focused → close. Cheap if the sidebar rebuilds state fast or persists it in `$HERDR_PLUGIN_CONFIG_DIR` ("the only place a plugin may keep user-editable settings" — beads `src/app.rs`).
2. **Width toggle** via `pane resize` deltas — keeps the process alive but is fiddly: relative amounts only, state tracked by us, and a skinny pane still occupies layout.
3. **Zoom** (`pane zoom --toggle/--on/--off`) is the *expand* direction, not collapse — useful as the sidebar's "maximize" affordance. (0.9.0 also has `plugin pane focus <PANE_ID>`, so the old zoom-on/off focus hack from file-viewer's 0.7-era launcher is no longer needed.)

### Other v1 constraints worth knowing

- "Runtime action registration and native non-terminal plugin UI are not part of plugin v1" — the sidebar is a terminal program declared statically in the manifest; Herdr's own built-in sidebar (`ui.*` config) is not extensible by plugins.
- Pane `command` must be `./`-prefixed relative (bare relative fails PATH resolution) and is unspawnable as a relative path on Windows (file-viewer manifest, verified against real hardware).
- Plugins launch with a minimal `PATH` and run in the plugin root, not the project — pass the target cwd via `--env` and/or query the focused pane's cwd (beads does both).
- `--no-focus` doesn't fully hold through split+swap; beads' auto-dock records and restores the user's focus afterward.
- `pane list` emits JSON on stdout by default (no `--json` flag) — beads discovers its own panes by OSC terminal-title markers in that output.

---

## Ranked placement options for the feedr sidebar

1. **`split` + swap-left + resize (the beads dock) — recommended.** The only option that yields an actual persistent left-edge pane. Real pane ID, focusable (`plugin pane focus`), resizable, per-tab. Limitations: per-tab (add an opt-in `tab.created` auto-dock hook for every-tab presence), relative-only sizing, collapse = close/reopen, not restored after server restart, focus-restore workaround needed.
2. **`tab`** — solid fallback "full view", not a sidebar. Per-workspace idempotency is proven (file-viewer). Good as a secondary action alongside the dock, not instead of it.
3. **`popup`** — great for a transient floating board (beads' 85%×85% pattern), but session-modal, singleton per session, no pane ID, no persistence, steals all keys. Disqualified as a sidebar; viable as a companion "expanded view".
4. **`overlay`** — transient inspection over the current pane with automatic focus/zoom restore. By design temporary; not persistent.
5. **`zoomed`** — full-tab on open; it's the opposite of a sidebar. Only useful if the primary interaction were "take over the tab".

## Recommendation

Adopt the beads dock pattern wholesale: manifest `[[panes]]` with `placement = "split"`, an idempotent per-tab launcher script (open → swap-walk left → `pane resize` shrink; focus if present; close if focused = collapse), keybinding via a `plugin_action`, and an optional `tab.created` auto-dock hook gated on a `$HERDR_PLUGIN_CONFIG_DIR` marker file for users who want the sidebar on every tab. Treat "collapse" as close/reopen with state persisted in the plugin config dir; offer `pane zoom` as the expand affordance.
