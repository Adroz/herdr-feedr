# Research: how the sidebar can tell it is alone in its tab

Ticket: [#20](https://github.com/Adroz/herdr-feedr/issues/20) (map: #14). Sources: herdr source at tag **v0.9.3** (paths below are relative to the herdr repo root), feedr at `48b7d8a`, and live probes against the user's herdr 0.9.3 server on 2026-10-28. The probes used two throwaway tabs (`w4:tA`, `w4:tB`) created and closed by the investigation; the user's focus was put back afterwards.

## TL;DR

- **Detect:** subscribe to `layout.updated`. It is a global (non-per-pane) subscription. Each event carries the whole layout of one tab: `tab_id`, and `panes[]` with ids. When the last other pane leaves, herdr emits `pane.closed`/`pane.exited`, then a `layout.updated` for our tab whose `panes` is just the sidebar. Verified live.
- **Confirm before acting:** after a short debounce, re-query (`pane.list`, filtered to our `tab_id`, or `tab.get` → `pane_count`). Close only on a positive answer that we are alone. Never close because a query failed.
- **Close:** just exit the process (`should_quit`). On unix, herdr's exit action for a non-respawning pane is `ClosePane`. If that leaves the tab with no panes, the tab is removed. If it was the workspace's last tab, the workspace closes. Verified live: pressing `q` in a lone sidebar removed its tab.
- **Correction to the ticket:** herdr does **not** exit when the last workspace closes. Since #366 it opens a fresh default workspace whenever a client is attached.
- **Existing bug found:** feedr's subscriber never reacts to pushed events. It checks `v["type"]` at the top level. The wire format is `{"event": "...", "data": {"type": "...", ...}}`.

## 1. Events on pane close / layout change

Event catalogue: `src/api/schema/events.rs:17-84` (subscriptions), `:422-555` (payloads).

| Event | Payload | Fires when |
|---|---|---|
| `pane.closed` | `{pane_id, workspace_id}`, no `tab_id` (`events.rs:497-500`) | `pane.close` / `plugin pane close` / Ctrl+B X (`src/app/api/panes.rs:1989-2013`) |
| `pane.exited` | `{pane_id, workspace_id}` (`events.rs:526-529`) | the pane's process exited (`src/app/api.rs:277-288`) |
| `layout.updated` | `{layout: PaneLayoutSnapshot}`: `workspace_id, tab_id, zoomed, area, focused_pane_id, panes[], splits[]` (`events.rs:552-554`, `src/api/schema/panes.rs:593-601`) | after a pane is removed **if the tab survives** (`panes.rs:2011-2013`, `api.rs:290-296,374-376`; target computed by `layout_update_target_after_pane_removal`, `panes.rs:2176-2194`, only when `pane_count > 1` before removal), and after split, swap, resize, move and similar operations |
| `tab.closed` | `{tab_id, workspace_id}` | `tab.close` and pane moves only. **Not** emitted when a tab dies because its last pane closed or exited. Confirmed live: closing the lone sidebar gave only `pane_closed`/`pane_exited`. |
| `pane.moved` | includes `previous_tab_id` and `closed_tab_id` | `pane.move`. The source tab also gets a `layout.updated` (`panes.rs:1355`). |

`layout.updated` is a plain subscription with no `pane_id` (`src/api/subscriptions.rs:161`). It avoids the trap that broke `pane.agent_status_changed`. It is excluded from manifest `[[events]]` hooks (`events.rs:286-309, 351-358`), so it is only available on a socket subscription. That is fine, because the sidebar keeps one open already.

What the live probe saw when the shell beside the sidebar was closed (trimmed):

```
{"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w4:p1H","workspace_id":"w4"}}
{"event":"layout_updated","data":{"type":"layout_updated","layout":{"tab_id":"w4:tB","focused_pane_id":"w4:p1J",
  "panes":[{"pane_id":"w4:p1J","focused":true,"rect":{...}}],"splits":[],"zoomed":false,...}}}
...then after `q` in the sidebar:
{"event":"pane_exited","data":{"type":"pane_exited","pane_id":"w4:p1J","workspace_id":"w4"}}
```

**What feedr subscribes to today:** `pane.created`, `pane.closed` and `pane.agent_detected` (`src/tui/socket.rs:382-386`). `pane.closed` does fire on the relevant closes, but:

1. it carries no `tab_id`, so the sidebar cannot tell whether the close happened in its own tab without a follow-up query; and
2. **the handler never runs.** `socket.rs:399-401` resyncs only `if v.get("type").is_some()`. Pushed events are serialised as `EventEnvelope {event, data}` (`events.rs:83-86`, written by `src/api/server.rs:941-951`). `type` sits inside `data`, so nothing at the top level matches. Agent statuses refresh only through the 3 s read-timeout resync. The unit test `subscribe_loop_resyncs_on_events` (`socket.rs:745-760`) feeds a fake top-level-`type` frame, which is why it passes. This should be fixed alongside the feature: match on `v.get("event")`.

## 2. Listing the panes in the sidebar's own tab

- `pane.list` takes only an optional `workspace_id` (`src/api/schema/panes.rs:307-310`). There is **no tab filter**. With no argument it returns every pane in every workspace (`src/app/creation.rs:163-199`; the CLI passes through only `--workspace`, `src/cli/pane.rs:60-79`). Each entry carries `tab_id` (`panes.rs:449-454`), so you filter on the client side, as `reload-feedr.sh` already does.
- It enumerates `tab.layout.pane_ids()`, meaning tiled and overlay panes (`creation.rs:183,195`). Overlay panes are real layout panes, shown with the tab zoomed (`src/app/custom_commands.rs:418-434`). The **popup** pane is held separately in `state.popup_pane` (`src/app/state.rs:877`) and is not listed.
- Cheaper count: `tab.get <tab_id>` returns `TabInfo.pane_count` (`src/api/schema/tabs.rs:46`). Live: `herdr tab get w4:tA` gave `pane_count: 2`, then `1` after the shell closed. A count cannot tell a second feedr sidebar apart from real company, though (see §4), so `pane.list` filtered by `tab_id` is the better confirm query.
- The `layout.updated` payload already lists the tab's pane ids. For the common case no extra query is needed to *detect* that the sidebar is alone. The query is only there to *confirm*.

**Own identity:**
- Every managed pane process gets `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID` and `HERDR_PANE_ID` at spawn (`src/pane.rs:187-197`, `src/integration/env.rs:8-10`). feedr already reads `HERDR_PANE_ID` (`src/tui/app.rs:192`).
- Treat `HERDR_TAB_ID` as a **hint only**. It is fixed at spawn, and the pane can be moved to another tab with `pane.move`, which changes its `tab_id`. Resolve the current tab live: take the `tab_id` from the `pane.list`/`pane.get` entry whose `pane_id == HERDR_PANE_ID`, or from any `layout.updated` whose `panes[]` contains our id.
- Pane public ids are per-workspace numbers (`w4:p1J`; `src/app/ids.rs:123-131`), so a move across workspaces changes the id. If our pane id stops resolving, do nothing (fail safe).
- Do not identify ourselves by the title marker. Several sidebars share `feedr-sidebar` (`src/tui/dock.rs:8`).

## 3. How the sidebar should close itself

**Recommended: exit the process.** Set `should_quit`, which is the existing `q` path (`src/tui/input.rs:11`). `ratatui::restore()` runs and the process exits.

- When a pane's child exits, herdr picks `runtime_exit_action` (`src/app/api.rs:509-523`). On unix that is `ClosePane` unless `respawn_shell_on_exit` is set (only imported or restored argv panes, `src/persist/restore.rs:685-689`). Plugin panes are not set to respawn.
- `handle_pane_died` (`src/app/actions.rs:2025-2080`) then calls `Workspace::remove_pane` (`src/workspace.rs:869-893`):
  - if the pane was the tab's only pane and the workspace has other tabs, the tab is removed;
  - if it was the workspace's only tab, the workspace is removed;
  - if no workspaces remain, the state becomes empty and the mode switches to Navigate.
- The headless server then calls `ensure_default_workspace` whenever a shell client is attached (`src/server/headless.rs:499, 1890, 3187-3188`; `src/app/mod.rs:698-`). CHANGELOG 0.7.x: "closing the last workspace now opens a default workspace instead of leaving the client on an empty screen" (#366). **herdr keeps running; it does not exit.**
- Verified live: lone sidebar, `herdr pane send-keys <id> q`, `pane_exited`, tab gone from `tab list`.

**Alternative: `pane.close` or `plugin pane close` on our own id.** Both go through the same `close_pane` (`src/app/api/plugins/mod.rs:579-601` → `panes.rs:1956-2017`), with the same tab and workspace effects (verified live with `herdr pane close`). Differences:

- It kills our own process from under us via the PTY, so there is no clean shutdown.
- It needs the socket to be reachable.
- It returns `confirmation_required` when `ui.confirm_close` is on and the close would take down a whole worktree group (`panes.rs:1966-1973`).

Process exit skips that check, but it also only ever removes the one workspace, never the group (`actions.rs:2048-2056`, compare `close_selected_workspace`/`workspace_close_indices`, `actions.rs:666-704, 881-899`). Use exit.

**Same behaviour after a user Ctrl+B X:**
- Ctrl+B X is the client keybinding `ClosePane` → `Method::PaneClose` (`src/client/shell/actions.rs:1053`). That is the same API close, so the sequence is the same `pane.closed` followed by `layout.updated`.
- A shell the user `exit`s takes the pane-died path, which also emits `layout.updated` (`api.rs:374-376`).
- Plugin panes need no special handling. `remove_plugin_pane_records` is called on both paths (`actions.rs:2027`, `panes.rs:1982`).

## 4. Edge cases to guard

1. **Fresh tab with auto-dock.** `tab.create` spawns the root shell *before* `tab.created` fires. Live order: `pane_created(shell)` → `layout_updated` → `tab_created` → `pane_created(sidebar)` → `layout_updated` ×3 (swap, resize, fit). The auto-docked sidebar is never alone at startup. A sidebar *can* start alone, though: launched as `--placement tab`, as the sole pane by hand, or by `reload-feedr.sh`'s fallback path. **Guard:** self-close only on a *transition*. Arm only after seeing at least one companion in our tab, and never close on the first snapshot.
2. **Close then open in quick succession** (a tool that closes a pane and re-splits, or a user who closes the shell to open another). **Guard:** debounce about 300-500 ms after the "alone" event, then confirm with a fresh `pane.list`. If anything arrived in the meantime, stay. The sidebar briefly fills the tab during the debounce, since herdr gives it the full width. This flash is cosmetic.
3. **herdr server restart.** `PluginPaneRecord`s live only in memory (`src/app/state.rs:12-15, 875`; nothing in `src/persist/` touches them). A restored sidebar slot comes back as a plain shell in the saved cwd (`restore.rs:683-689`), so no feedr process is left to self-close. The revived shell counts as genuine company, which is correct because it is a real shell. During shutdown the socket disappears or panes die. **Guard:** never treat a failed query, `SocketDown`, or an `events_lost` error (`src/api/subscriptions.rs:329-339`) as "alone".
4. **Collapsed rail.** Collapsing only resizes the pane (`src/tui/dock.rs:609-660`). It is still one layout pane, so detection is unaffected. A collapsed lone sidebar should close too, and no extra handling is needed.
5. **Other feedr sidebars in the same tab.** `reload-feedr.sh` briefly has two in one tab, but alongside a shell. **Recommend:** a pane whose `terminal_title_stripped == "feedr-sidebar"` is *not* company. Otherwise two leftover sidebars keep each other alive forever. If both see "alone (excluding sidebars)" they both exit, and the tab closes. That is fine.
6. **Other plugins' panes** (file-viewer split, an overlay). These are user content, so **count them as company**. pane.list cannot tell a plugin pane from any other except by title or label (`PaneInfo` has no plugin field, `panes.rs:449-484`).
7. **Zoomed tab / overlay / popup.** Zoom is a tab flag; every pane stays in the layout and in pane.list, so a zoomed companion still counts. An overlay is a real layout pane and counts. A popup is not in the layout and does not count. That is correct, because a popup floats over the session, not over this tab.
8. **The sidebar moved to another tab (`pane.move`).** `HERDR_TAB_ID` is now stale. Re-resolve the tab from our `pane_id` each time (§2). If our id no longer resolves (moved across workspaces), do nothing.
9. **Other tabs moving the shell away** (`pane.move` of the companion to another tab). The source tab gets `layout.updated` with just the sidebar, so the sidebar closes. That matches the decided behaviour.
10. **Worktree group parent workspace.** Exiting closes only this workspace, not the group, and skips the `confirm_close` prompt that `pane.close` would raise. Acceptable, but see the decision below.

## Recommended mechanism

1. In `subscribe_loop`, add `{"type": "layout.updated"}` and fix the dispatch to key on `v["event"]`. Forward `layout_updated` events whose `data.layout.panes[]` contains our pane id to the app as a new `AppEvent::OwnTabLayout { tab_id, pane_ids }`.
2. Keep company tracking in the app. "Company" means other panes in our tab, excluding other `feedr-sidebar` panes (titles come from the confirm query).
   - Arm once company has been seen.
   - When an armed sidebar sees a layout with no company, start a debounce of about 400 ms.
   - When the debounce ends, run `pane.list` filtered to the `tab_id` of our live entry.
   - Still no company and the query succeeded: set `should_quit`.
   - Any error, or company found: stay, and stay armed.
3. Put the decision in a pure function, e.g. `fn should_self_close(own: &str, tab_panes: &[PaneEntry], armed: bool) -> bool`, so it can be unit-tested without herdr.

## Decisions left for the human

- **Closing the workspace:** should the sidebar self-close when its tab is the workspace's *only* tab? That closes the whole workspace. herdr then opens a default workspace if it was the last one, and with `auto_dock` that new workspace's tab may get a sidebar again (not verified). Recommendation: yes, because it mirrors the double Ctrl+B X. Optionally skip it when the workspace is a worktree-group parent.
- **Other plugins' panes:** confirm they count as company (recommended), and that other feedr sidebars do not (recommended).
- **Opt-out:** should there be a config flag, e.g. `[sidebar] auto_close = true` by default, for users who want a lone sidebar to stay?

## Side findings (separate tickets)

- **Broken event dispatch:** see §1 (`socket.rs:399-401`). Fixing it also makes agent statuses push-driven instead of the 3 s poll.
- **Auto-dock moves focus:** in both live probes, `tab.create --no-focus` left the user's focus alone, but a moment later the `on-tab-created` auto-dock moved the user's focused workspace and active tab to the new tab. The likely culprit is the swap-walk or the resize in `open_split_dock`. This matches the known gotcha that `pane swap` switches tabs.
- **Workspace-wide scripts:** `open-feedr.sh` / `toggle-feedr.sh` `find_sidebar_pane` run `herdr pane list` with no `--workspace`, so they match a sidebar in *any* workspace, not "this workspace" as their headers claim.
