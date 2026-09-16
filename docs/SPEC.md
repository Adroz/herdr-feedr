# herdr-feedr — v1 Specification

The feed rack for your herd: a markdown-backed to-do sidebar for [Herdr](https://herdr.dev), shared between you and your coding agents. You keep a plain to-do list; agents pull work from it, add their own, and report back — all through one file you can always open in any editor.

Decided via the wayfinder map ([#1](https://github.com/Adroz/herdr-feedr/issues/1)); each section links its ticket.

## 1. The feed file ([#5](https://github.com/Adroz/herdr-feedr/issues/5), [#9](https://github.com/Adroz/herdr-feedr/issues/9))

One **global** markdown file. Default `~/.config/herdr-feedr/feed.md`; overridable with `feed_path` in plugin config. Per-project feeds are a future direction, not v1.

Format — a GitHub-flavored markdown checklist:

```markdown
# Feed

- [ ] Fix auth redirect loop
  Repro: expired session → login → bounces forever.
  Suspect token expiry check uses `<` not `<=`.
- [~] Migrate CI to GitHub Actions @agent(claude:0198f3ab-7c2e-4b8d)
  Keep CircleCI config until parity confirmed.
- [x] Bump Node to 22

## Later

- [ ] Evaluate pnpm catalogs

## Agent

- [?] Add retry to flaky deploy test @agent(claude:0198f3ab-7c2e-4b8d)
  Done: PR #12. Two green runs.
```

- **Item** = `- [<state>] Title` line. **States**: `[ ]` open · `[~]` in progress · `[?]` awaiting review · `[x]` done.
- **Title** = rest of the line minus trailing `@key(value)` tokens. The sidebar shows only this line.
- **Body** = following lines indented two spaces: freeform markdown (fenced code allowed, keep the indent). Context for whoever works the item; hidden in the sidebar list.
- **Agent link** = trailing `@agent(<kind>:<session-id>)` token. One per item; the latest claim replaces it.
- **Sections** = `##` headings, optional, in your order. Top-to-bottom is your priority order.
- **Archive** = a `# Done` region at the bottom of the file, holding swept items under mirrored `##` section names, bodies kept, each stamped `@done(YYYY-MM-DD)` (see §2a).
- The file is always valid markdown a human can edit directly.

## 2. Provenance and completion authority ([#7](https://github.com/Adroz/herdr-feedr/issues/7))

- **Human-created** items live in your sections. Only you write their `[x]` — even when an agent did the work. An agent finishing one writes `[?]` plus a body evidence note (PR link, test results); you review and tick.
- **Agent-created** items live in the reserved `## Agent` section. Agents may close them `[x]` themselves.
- Provenance = who *initiated*, not who typed: "add X to my list" said to an agent lands in your sections with your authority.

## 2a. Sweep and archive ([#10](https://github.com/Adroz/herdr-feedr/issues/10))

Sweeping clears completed items from the active list while keeping history:

- **Manual only** — the sidebar's broom button or `feedr sweep`. Only `[x]` items move; `[?]` stays (awaiting review). No auto-sweep.
- Swept **human-created** items move to the `# Done` region (mirrored section names, bodies kept, `@done(YYYY-MM-DD)` stamped).
- Swept **agent-created** items are **deleted**, not archived.

## 3. The sidebar ([#2](https://github.com/Adroz/herdr-feedr/issues/2), [#6](https://github.com/Adroz/herdr-feedr/issues/6))

A ratatui TUI in a herdr plugin pane, docked with the herdr-beads pattern: open as `split`, swap-walk to the left edge, resize narrow. Herdr plugin v1 has no native dock, so:

- The pane is **per-tab** (leftmost within the tab's pane grid; herdr's own sidebar is app chrome and can sit further left).
- `auto_dock = false` by default; `true` docks the sidebar on every `tab.created`.
- The launcher is idempotent (reopen ≡ focus), which also recovers the pane after a herdr server restart (plugin panes revive as plain shells).

Rendering: sections as headers; one line per item — state glyph + title, ellipsized under ~20 cols; agent-linked items show an `@agent` sub-line with a **live status glyph** (working/blocked/done from herdr socket events — display only, never written to the file).

Interactions:

- **Checkbox click** advances state: `[ ]`→`[~]`→`[x]`→`[ ]`. On a `[?]` item, click = accept → `[x]`. Clicks never produce `[?]` (that's the agent's signal).
- **Title click** opens an edit modal (TUI overlay): title and body both editable.
- **`+` row / `a`** opens the same modal empty — Category (topmost, with a suggestion
  dropdown over existing and archived section names; free text creates a new `##`
  section), title, optional body. An empty Category lands the item at the end of the
  first human section. Editing an item's Category moves it to the end of the target
  section; clearing it moves the item to the uncategorized region. Reserved names
  (Agent, Done, Feed) are rejected. Delete lives in the modal, with confirm. See
  `docs/superpowers/specs/2026-09-16-category-field-design.md`.
- **`e`** opens the feed file in `$EDITOR` for bulk edits and reordering.
- **`@agent` sub-line click**: pane open → focus it (`agent.focus`); pane gone → open a new tab resuming that session (kind prefix selects the command, e.g. `claude --resume <id>`).
- **`«` button** collapses to a ~3-col rail showing `»`; click restores the previous width (implemented as resize — no hide-without-close in plugin v1). Native drag-resize works as on any pane.
- **Broom button** runs a sweep (§2a).
- **Bottom-pinned `Done (n)` row**: click opens a read-only modal viewing the `# Done` archive.
- **File button**: view the feed internally (plaintext modal, read-only) or open it externally in `$EDITOR`.
- **`toggle-feedr`** plugin action (user-bindable) fully closes/opens the sidebar.
- Refresh is automatic: file-watch on the feed + socket event subscription. No refresh key. The sidebar never steals focus on open.

## 4. The `feedr` CLI ([#8](https://github.com/Adroz/herdr-feedr/issues/8))

The same binary as the TUI. Subcommands are the machine interface to the feed:

`feedr list` · `feedr show <item>` · `feedr claim <item>` · `feedr add [--agent-owned] <title>` · `feedr review <item>` · `feedr done <item>` · `feedr sweep`

- `claim` writes `[~]` and stamps `@agent(<kind>:<session-id>)`.
- `review` writes `[?]` (+ the evidence note comes from the body the agent appends); `done` writes `[x]` — permitted only per §2 authority.
- All writes go through one parser/writer shared with the sidebar (see §6).

## 5. The skill ([#4](https://github.com/Adroz/herdr-feedr/issues/4), [#8](https://github.com/Adroz/herdr-feedr/issues/8))

`skills/herdr-feedr/SKILL.md` — the agent's interface to the feed, expressed as "run the `feedr` CLI":

- Agents normally arrive with context (a named task, or a task to create). A context-free invocation lists open items and **asks**; it never auto-grabs.
- Claim before working; finish per §2 (`review` + evidence on human-created, `done` on agent-created); add follow-up work with `add --agent-owned`.
- Delivery (herdr has no skill mechanism; plugin `skills/` dirs are inert convention): primary install `npx skills add Adroz/herdr-feedr -g`; the plugin build step copies (never symlinks) the skill over installed copies so herdr's reinstall-to-update flow refreshes it; AGENTS.md paste-in documented as fallback. The skill is version-stamped and consistency-tested against the CLI.

## 6. Concurrency ([#8](https://github.com/Adroz/herdr-feedr/issues/8), map fog)

You and agents edit the same file at the same time; that's the point. How nothing gets lost:

- Every sidebar action and every CLI command **reads the file fresh, applies its one change, and writes the whole file back atomically** (temp file + rename — the file on disk is never half-written). Interleaved small edits — you ticking a box while an agent claims an item — all land.
- The sidebar re-reads on every file change, so it always shows current state.
- The one loseable case: a whole-file `$EDITOR` session held open while something else writes — the editor's save wins over anything written since the buffer opened. Accepted for v1; keep editor sessions short. Everyday edits go through the safe path.

## 7. Agent status plumbing ([#3](https://github.com/Adroz/herdr-feedr/issues/3))

From the herdr socket API (`~/.config/herdr/herdr.sock`, NDJSON): `pane.list`/`agent.list` for agent kind, status, and `agent_session` UUID; `events.subscribe` for live status changes; `agent.focus` for navigation. Task-plan transcript replay (TaskCreate/TaskUpdate) is **not needed in v1** — agents surface their work by writing to the feed via the CLI; the socket supplies only statuses, the join key, and focus.

## 8. Implementation ([#9](https://github.com/Adroz/herdr-feedr/issues/9))

Rust. One static binary: ratatui sidebar + `feedr` CLI. Packaging copies the plugin-ecosystem conventions (`herdr-plugin.toml` manifest, fetch-or-build script, platform matrix) with repo topics carrying `todo`/`task` keywords for registry search. Theming: none in v1 — follow the terminal palette.

## 9. Out of scope / future

- Herdr core changes.
- Per-project feed scoping/sectioning (future direction; v1 is global-only).
- Sidebar-initiated "send item to agent" (spawn) — v2 candidate; v1 links are created by claims.
- Drag-reorder in the TUI (reorder via `$EDITOR`).
- Task-plan transcript mirroring.
