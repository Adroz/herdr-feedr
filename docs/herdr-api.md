# herdr API facts

Behaviour of herdr's CLI and socket that feedr depends on, verified live against herdr 0.9.3. `herdr api schema --json` gives shapes; this file holds what the schema doesn't say. Re-verify after a herdr upgrade.

## Layout and panes

- `layout.apply` (socket) is **destructive**: it builds a new tab with brand-new panes even when leaves name existing `pane_id`s, so the old processes die. Never run it on a tab holding agents.
- `layout.set_split_ratio {tab_id, path, ratio}` sizes a split exactly. `path` is bools from the root: `false` = first child, `true` = second. It doesn't activate the tab. feedr's `plan_fit` uses it.
- `pane swap` focuses the **source** pane and activates its tab. A self-swap is a no-op.
- `pane resize --amount` is a delta share of the nearest split, not of the tab width.
- `pane focus` and `tab create` (even without `--focus`) switch the user's visible tab.
- The CLI can't focus an ordinary pane by id; use the socket's `pane.focus`.
- `pane.list` marks exactly one pane `focused: true` across all workspaces.
- `pane.list`'s `PaneInfo` has **no plugin identity and no zoom/overlay flag**. Zoomed and overlay panes are listed like any other; popups are never listed. feedr recognises its sidebar by the terminal title it sets on itself, `feedr-sidebar` (`terminal_title_stripped`, falling back to `terminal_title`).
- `server.stop {}` shuts herdr down. herdr otherwise opens a fresh default workspace when the last pane closes.

## Event subscriptions

- Pushed frames are `{"event": …, "data": {…}}`. Global events (`pane_closed`, `layout_updated`, …) repeat the type as `data.type`. Per-pane events use the dotted subscription name (`pane.agent_status_changed`) and have **no** `data.type`. Accept both.
- `pane.agent_status_changed` is **per pane**: subscribe once per agent pane, or no status pushes arrive.
- A second `events.subscribe` on the same connection makes herdr **close the connection**. To change the subscription set, open a new connection.
- `layout.updated` carries the tab id and its pane ids.
