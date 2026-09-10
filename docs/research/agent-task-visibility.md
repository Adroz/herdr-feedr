# Research: what herdr exposes about running agents' task lists

Ticket: [#3](https://github.com/Adroz/herdr-feedr/issues/3) · Investigated against herdr **0.9.0** (protocol 22) — live local server, `herdr --skill`/`--help`/subcommand output, `herdr api schema`, [herdr.dev docs](https://herdr.dev/docs), and the [aclima01/herdr-todos-windows](https://github.com/aclima01/herdr-todos-windows) plugin source. All payloads below were verified against the live socket at `~/.config/herdr/herdr.sock` unless noted.

## TL;DR

- **Pane/agent listing + status**: first-class. `pane.list` / `agent.list` return every pane with `agent_status` ∈ `idle | working | blocked | done | unknown`, plus the pane's terminal title (which carries the agent's current activity) and — crucially — the agent's **session id**.
- **Push updates**: first-class. `events.subscribe` streams `pane.agent_status_changed` (and pane/tab/workspace lifecycle) over the same socket connection.
- **Task-plan events (TaskCreate/TaskUpdate)**: **not exposed by the socket API.** No task/todo event type exists in the 0.9.0 schema. herdr-todos-windows gets them by reading the Claude Code **session transcript on disk**, joined via the session id herdr reports per pane. feedr must do the same.
- **Jump-to-agent**: first-class. `agent.focus` / `pane.focus` / `tab.focus` / `workspace.focus` switch the UI to a target; one CLI call: `herdr agent focus <pane_id|name>`.

## 1. Transport

Source: [Socket API docs](https://herdr.dev/docs/socket-api/) (mirrored at `docs/next/website/src/content/docs/socket-api.mdx` in herdrdev/herdr, tag v0.9.0); confirmed by `herdr status` and `herdr api schema`.

- **Newline-delimited JSON over a local Unix domain socket** (named pipe on Windows). Default session: `~/.config/herdr/herdr.sock`; named sessions: `~/.config/herdr/sessions/<name>/herdr.sock`.
- Request envelope, one per line: `{"id":"req_1","method":"pane.list","params":{}}`. Responses echo the `id`; errors look like `{"id":"req_1","error":{"code":"not_found","message":"pane not found"}}`.
- No auth; access control is filesystem permissions on the socket.
- The `herdr` CLI is a thin wrapper over the same methods and prints the raw JSON response (`herdr pane list` → `{"id":"cli:pane:list","result":{...}}`), so a plugin can shell out to `$HERDR_BIN_PATH` or speak raw JSON to `$HERDR_SOCKET_PATH` — both env vars are injected into plugin processes ([plugin docs](https://herdr.dev/docs/plugins/)).
- The full machine-readable method/schema catalog comes from `herdr api schema --json` (`schema_version: 1`, `protocol: 22`). `herdr api snapshot` dumps live runtime state.

## 2. Listing panes/agents and their statuses

Methods (from `herdr api schema`): `pane.list`, `pane.get`, `pane.current`, `agent.list`, `agent.get`, `workspace.list`, `tab.list`. CLI: `herdr pane list [--workspace ID]`, `herdr agent list`, etc.

Verified live `pane.list` entry (herdr 0.9.0, Claude Code pane):

```json
{
  "pane_id": "w1:p7", "tab_id": "w1:t5", "workspace_id": "w1",
  "terminal_id": "term_65b1730247a491",
  "agent": "claude",
  "agent_status": "done",
  "agent_session": {"agent": "claude", "kind": "id", "source": "herdr:claude",
                    "value": "13bb6c2a-1b44-4485-a9c3-e02f5d662dfd"},
  "cwd": "/Users/nikmoores/stile/dev-environment",
  "foreground_cwd": "/Users/nikmoores/stile/dev-environment",
  "focused": false,
  "terminal_title": "✳ documentation issue 17914",
  "terminal_title_stripped": "documentation issue 17914",
  "revision": 3,
  "scroll": {"max_offset_from_bottom": 0, "offset_from_bottom": 0, "viewport_rows": 62}
}
```

Field notes for feedr:

- `agent_status`: schema enum is exactly `["idle","working","blocked","done","unknown"]`. Panes with no recognized agent have `agent_status: "unknown"` and omit `agent`/`agent_session`.
- **`idle` vs `done`** (per `herdr --skill`): both mean ready-for-input; the server distinguishes them by *seen* state. Explicit focus commands mark the target seen (done → idle); reads do not. `blocked` means herdr recognized an approval/question UI — the "waiting on you" signal feedr cares about. `unknown` does not prove completion.
- `agent_session.value` is the agent's **native session id** (for Claude Code, the session UUID used in transcript filenames). It is reported to herdr by the agent-side integration (`herdr integration install claude`, which uses `pane.report_agent_session` under the hood). This is the join key from a herdr pane to on-disk task data (§4).
- `terminal_title_stripped` is the agent's live "what I'm doing now" line (Claude Code sets the terminal title per turn) — herdr-todos-windows uses it verbatim for its "Now" display.
- `agent.list` returns the same shape restricted to panes hosting recognized agents, plus `state_change_seq` (monotonic per-server state-change counter — useful for ordering/dedup).
- Named agents: a live agent can carry a unique name (`herdr agent rename`); agent methods accept either that name or the hosting `pane_id` as `target`.

## 3. Push events: `events.subscribe`

Source: socket-api docs + `herdr api schema` (`EventsSubscribeParams`, `Subscription`, event schema).

```json
{"id":"sub_1","method":"events.subscribe",
 "params":{"subscriptions":[{"type":"pane.agent_status_changed"}]}}
```

The connection stays open after the initial response; each subsequent line is a pushed event. The `pane.agent_status_changed` subscription accepts optional filters `pane_id` and `agent_status` (e.g. only `blocked`). Verified pushed-event payload shape:

```json
{"type": "pane_agent_status_changed", "pane_id": "w1:p7", "workspace_id": "w1",
 "agent_status": "working", "agent": "claude",
 "display_agent": null, "title": "...", "state_labels": {}}
```

Full subscription-type catalog in 0.9.0 (from `Subscription` in the schema — note **no task/todo types**):

`workspace.created|updated|metadata_updated|renamed|moved|reordered|closed|focused`, `worktree.created|opened|removed`, `tab.created|closed|focused|renamed|moved`, `pane.created|closed|updated|focused|moved|exited|agent_detected|agent_status_changed`, plus a pane-output match subscription (`pane_id` + `match` + optional `lines` — substring/regex on pane output).

`events.wait` is the one-shot variant (`match_event` + optional `timeout_ms`); CLI equivalents include `herdr agent wait <target> --until blocked` and `herdr pane wait-output`.

Recommended feedr wiring: one long-lived socket connection subscribing to `pane.agent_status_changed` + `pane.created` + `pane.closed` + `pane.agent_detected`, with `pane.list` on startup/reconnect to resync (using `revision`/`state_change_seq` to discard stale info).

## 4. Task-plan visibility (TaskCreate/TaskUpdate): NOT in the socket API — read the transcript

**Finding**: the 0.9.0 schema contains no method or event exposing an agent's task list. Searching every method (`herdr api schema`) and every subscription/event type confirms it: herdr models *agent lifecycle state*, not *task plans*.

How [herdr-todos-windows](https://github.com/aclima01/herdr-todos-windows) does it (README + `todo-panel.ps1`), which is the pattern feedr should reuse:

1. **Find the agent**: `herdr pane list --workspace $ws` → pick the pane sharing the tab; read `agent`, `agent_status`, and `agent_session.value` (requires the Claude integration installed so the session link exists).
2. **Locate the transcript**: Claude Code writes `~/.claude/projects/**/<session-id>.jsonl`; the panel globs for `<session-id>.jsonl` under that root.
3. **Replay task tool calls** from the transcript: `TaskCreate` entries in order get ids `1..N`; `TaskUpdate` sets `taskId → status`; an `in_progress` task's `activeForm` is the present-continuous label. It also handles the **older `TodoWrite` scheme** (full-list snapshots) — both exist across Claude Code versions (`todo-panel.ps1` line ~126).
4. **Poll, don't subscribe**: incremental reads with a byte high-watermark; transcript path change or file shrink resets state. Live status + "Now" line come "free from `herdr pane list`".
5. **Plan fallback**: if no tasks yet but the transcript names an approved plan file (`~/.claude/plans/*.md`), mirror its checkbox/numbered steps until real tasks appear.

Implications for feedr:

- Task-level feed items require **per-agent-kind transcript adapters**; the herdr socket only supplies the join key (`agent_session`) and the pane/status context. Claude Code is fully covered by the pattern above; other agent kinds need their own session-file formats (herdr's `agent_session.source` tags the integration, e.g. `herdr:claude`).
- File-watch/poll the JSONL; there is no push channel for task changes.
- `pane.report_metadata` (and `workspace.report_metadata`) let a cooperating process attach freeform `--token NAME=VALUE` metadata (with TTL) to a pane — a possible future channel for richer status, but nothing populates it with tasks today.

## 5. Focusing a pane programmatically (click-item → jump-to-agent)

First-class, at every level (schema-verified methods and params):

| Method | Params | CLI |
|---|---|---|
| `agent.focus` | `{"target": "<name-or-pane_id>"}` | `herdr agent focus <target>` |
| `pane.focus` | `{"pane_id": "w1:p7"}` | (socket; CLI exposes directional `herdr pane focus --direction ...`) |
| `tab.focus` | `{"tab_id": "w1:t5"}` | `herdr tab focus <tab_id>` |
| `workspace.focus` | `{"workspace_id": "w1"}` | `herdr workspace focus <id>` |
| `plugin.pane.focus` | `{"pane_id": ...}` (plugin-owned panes) | `herdr plugin pane focus` |

`agent.focus` on a pane in another workspace/tab is the single-call jump. Side effect worth knowing: explicit focus marks the agent *seen*, collapsing `done` → `idle` — exactly the semantics a feed wants when the user clicks through. Also useful: `pane.zoom`, and `herdr notification show <title> [--sound done|request]` for feed-side alerts.

## 6. Packaging feedr as a herdr plugin

Source: [plugin docs](https://herdr.dev/docs/plugins/) + the todos-windows manifest.

- Manifest `herdr-plugin.toml`: `id`, `name`, `version`, `min_herdr_version`, `platforms`, plus `[[panes]]` (placement: `split | tab | overlay | popup | zoomed`), `[[actions]]` (UI/CLI-invokable, with `contexts`), `[[events]]` hooks (manifest-level triggers, e.g. `on = "pane.agent_detected"`, `on = "worktree.created"`), link handlers, and startup hooks.
- Plugin processes receive `HERDR_ENV=1`, `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ROOT`, `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_STATE_DIR`, `HERDR_PLUGIN_CONTEXT_JSON`, and the caller's `HERDR_WORKSPACE_ID` / `HERDR_TAB_ID` / `HERDR_PANE_ID` where applicable.
- Install/dev loop: `herdr plugin install <owner>/<repo>` (GitHub marketplace) or `herdr plugin link <path>` for local dev; `herdr plugin pane open|focus|close`, `herdr plugin action list|invoke`, `herdr plugin log list`.

## The concrete API surface feedr can rely on

1. **Inventory + status**: `pane.list` / `agent.list` → `pane_id`, `workspace_id`, `tab_id`, `agent`, `agent_status` (`idle|working|blocked|done|unknown`), `agent_session.value`, `terminal_title_stripped`, `cwd`, `focused`, `revision`, `state_change_seq`.
2. **Live updates**: `events.subscribe` on `pane.agent_status_changed` (+ pane lifecycle types) over the ND-JSON unix socket; resync with `pane.list`.
3. **Task detail**: not from herdr — join `agent_session.value` to `~/.claude/projects/**/<session-id>.jsonl` and replay `TaskCreate`/`TaskUpdate` (and legacy `TodoWrite`), with `~/.claude/plans/*.md` as fallback; poll with a byte high-watermark.
4. **Jump-to-agent**: `agent.focus {target}` (or `pane.focus`/`tab.focus`/`workspace.focus`); focus also marks `done` as seen.
5. **Packaging**: `herdr-plugin.toml` pane (`split`/`tab` placement) + `pane.agent_detected` event hook; talk to the socket via injected `HERDR_BIN_PATH`/`HERDR_SOCKET_PATH`.

Caveats: IDs and agent names are scoped to one server (multi-machine setups need per-host rediscovery); client/server versions can differ after updates — check `herdr status` before relying on newer methods; `unknown` status must not be rendered as "done".
