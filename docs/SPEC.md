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
- **Today** = trailing `@today` token ([#22](https://github.com/Adroz/herdr-feedr/issues/22)): the human's current focus. At most three uncompleted (`[ ]`/`[~]`/`[?]`) items carry it; the item stays in its home section. Any transition to `[x]` strips it in the same write, so the file keeps no trace of past focus. A hand-edited file with more than three is honoured, never rewritten.
- **Sections** = `##` headings, optional, in your order. Top-to-bottom is your priority order.
- **Archive** = a `# Done` region at the bottom of the file, holding swept items under mirrored `##` section names, bodies kept, each stamped `@done(YYYY-MM-DD)` (see §2a).
- The file is always valid markdown a human can edit directly.
- feedr never silently changes formatting it didn't need to touch: `[X]` reads as done and is kept as typed unless that item changes; CRLF and a missing trailing newline are preserved; tab-indented bodies are accepted.

## 2. Provenance and completion authority ([#7](https://github.com/Adroz/herdr-feedr/issues/7))

- **Human-created** items live in your sections. Only you write their `[x]` — even when an agent did the work. An agent finishing one writes `[?]` plus a body evidence note (PR link, test results) via `feedr review --note`; you review and tick.
- **Agent-created** items live in the reserved `## Agent` section. Agents may close them `[x]` themselves.
- Provenance = who *initiated*, not who typed: "add X to my list" said to an agent lands in your sections with your authority.
- **Today** is set and unset by the human only (edit modal or hand edit). Agents read it; no CLI command writes it.

## 2a. Sweep and archive ([#10](https://github.com/Adroz/herdr-feedr/issues/10))

Sweeping clears completed items from the active list while keeping history:

- **Manual only** — the sidebar's broom button or `feedr sweep`. Only `[x]` items move; `[?]` stays (awaiting review). No auto-sweep.
- Swept **human-created** items move to the `# Done` region (mirrored section names, bodies kept, `@done(YYYY-MM-DD)` stamped).
- Swept **agent-created** items are **deleted**, not archived.

## 3. The sidebar ([#2](https://github.com/Adroz/herdr-feedr/issues/2), [#6](https://github.com/Adroz/herdr-feedr/issues/6))

A ratatui TUI in a herdr plugin pane, docked with the herdr-beads pattern: open as `split`, swap-walk to the left edge, resize narrow (capped at a configurable max_width, default 46 columns). Herdr plugin v1 has no native dock, so:

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

Additions ([#22](https://github.com/Adroz/herdr-feedr/issues/22)):

- **Today panel**: a bordered ` Today ` block pinned above the list, no count; each item is a checkbox + title row with its category on an indented muted sub-line. Today items appear only there, in file order. Hidden when empty; outline turns red when over cap (hand edit). The edit modal shows `☆/★ Today` in its top border, toggled by Ctrl+T; a fourth mark is refused with "Today full — unmark one first".
- **Outside click**: closes read-only modals; closes a clean edit modal like Esc; a dirty edit modal stays open with "unsaved — Esc to discard, Ctrl+S to save"; an open dropdown closes first; ConfirmDelete cancels. The click never falls through to the list.
- **Copy**: feedr owns copy-on-select in edit fields and the read-only viewers (not the list). Mouse-up after a drag copies; double-click selects a word (herdr's rules) and triple-click a logical line, both copying immediately; Ctrl+C still works. Writes OSC 52, then pbcopy/wl-copy/xclip; status "Copied N chars". Cmd+C is not available (macOS terminals swallow it); herdr copy mode (`prefix+[`) is the fallback.
- **Paste**: bracketed paste; newlines become spaces in Title/Category, kept in Body.
- **Auto-close**: when the sidebar is the only pane left in its tab (other feedr sidebars/popups don't count as company), it exits, closing the tab and, if last, the workspace. If it was the last pane anywhere in herdr it calls `server.stop` so herdr quits. Armed only after company is seen; ~400 ms debounce plus a `pane.list` re-check; never closes on a failed query. `[sidebar] auto_close` and `quit_herdr_when_last` (both default true) opt out.
- Auto-dock restores the user's focus after placing a sidebar.

## 4. The `feedr` CLI ([#8](https://github.com/Adroz/herdr-feedr/issues/8))

The same binary as the TUI. Subcommands are the machine interface to the feed:

`feedr list` · `feedr show <item>` · `feedr whoami` · `feedr claim <item>` · `feedr unclaim <item>` · `feedr add [--agent-owned] <title>` · `feedr review <item> --note <line>` · `feedr done <item>` · `feedr sweep` · `feedr skill install|status`

- `claim` writes `[~]` and stamps `@agent(<kind>:<session-id>)`. The agent ref resolves in order: `--agent <kind>:<id>` > `FEEDR_AGENT` > `herdr pane current` (only when `HERDR_ENV=1`) > `CLAUDE_CODE_SESSION_ID` → `claude:<uuid>` — the last verified identical to herdr's `agent_session.value`, so the common case needs no socket call and identity also resolves outside herdr. When none yields an id, `claim` fails non-zero naming the sources tried; it never claims untagged, because an unlinked claim is a dead link the sidebar can't jump from.
- `whoami` prints the resolved ref and its source.
- `list` and `show` flag Today items. Item lookup prefers an exact case-insensitive title match over substring matches.
- `unclaim` releases a claim: it clears the `@agent` tag and returns the item to `[ ]`, so it reads as takeable again. Only an **in-progress** item can be released — once work has been handed back (`[?]`) or closed (`[x]`), the tag is the record of who did it, not a lock. Authority mirrors §2: an agent releases only its own claim; `--as-human` (what the sidebar uses) releases anyone's.
- `review` writes `[?]`; `done` writes `[x]` — permitted only per §2 authority. Both take a repeatable `--note <line>` that appends evidence to the item's body in the same write as the state change; `--note` is **required** on `review`, since `[?]` without its reason is what §2 exists to prevent.
- All writes go through one parser/writer shared with the sidebar (see §6).

## 5. The skill ([#4](https://github.com/Adroz/herdr-feedr/issues/4), [#8](https://github.com/Adroz/herdr-feedr/issues/8), [#12](https://github.com/Adroz/herdr-feedr/issues/12))

`skills/herdr-feedr/SKILL.md` — the agent's interface to the feed, expressed as "run the `feedr` CLI":

- Agents normally arrive with context (a named task, or a task to create). A context-free invocation lists open items and **asks**; it never auto-grabs.
- Claim before working; finish per §2 (`review` + evidence on human-created, `done` on agent-created); add follow-up work with `add --agent-owned`.
- A context-free invocation lists Today items first and suggests them.
- Body, in order: the golden rule (**never edit the feed file directly** — every read and write goes through the CLI, or §6's atomicity is void); identity via `feedr whoami`, stopping to ask if it fails rather than claiming untagged; claim-or-ask; `add --agent-owned` for follow-up work; finish by §2 authority (`--human` is off-limits to agents); `sweep` and the `# Done` archive are human-only; one worked example.
- Delivery (herdr has no skill mechanism; plugin `skills/` dirs are inert convention): the **binary owns it**. `skills/herdr-feedr/SKILL.md` is the in-repo source of truth, embedded with `include_str!` so `feedr skill install` works from an installed binary with no checkout.
  - `feedr skill install` writes exactly two paths and no others: `~/.agents/skills/herdr-feedr/` (canonical) and `~/.claude/skills/herdr-feedr/` (symlink into `~/.agents` when absent, copy when symlinking fails, overwrite when a real dir is already there). It prints every path written.
  - `scripts/build.sh` runs `feedr skill install --refresh-only`, which refreshes copies that already exist and **never creates** them — so herdr's reinstall-to-update flow refreshes an installed skill, while `herdr plugin install` never plants files in `$HOME` unasked.
  - First install stays explicit and is documented two ways: `npx skills add Adroz/herdr-feedr -g` (agent-neutral primary) or `feedr skill install` (no Node needed). An AGENTS.md paste-in block is the fallback for runners without skill support.
- Versioning: one number across crate, `herdr-plugin.toml`, and skill, stamped as a visible body line (`> Skill version 0.1.0 — requires feedr CLI >= 0.1.0.`) rather than a frontmatter key. `feedr skill status` reports each installed copy's path, stamped version, binary version, and OK/STALE; there are no ambient drift warnings. `tests/skill.rs` asserts that every `feedr ...` line in the skill parses against the real clap command, that every non-hidden subcommand appears in the skill, and that the three versions match.

## 5a. Claim liveness and release (sidebar)

- The `@agent` sub-line shows a **live marker** when that session appears in the herdr socket's agent list. Presence *is* liveness; the reported status only refines the glyph, so a live agent reporting an unknown status still reads as live. Absence of a marker asserts **nothing** — an agent working outside herdr, or a claim predating a herdr restart, is indistinguishable from a dead one, and the sidebar must not label either as dead.
- `feedr list` and `feedr show` print the same signal as text: a claimed item's tag reads `@claude:abc (live)` when the socket confirms that session. Agents read the feed through the CLI and never see the sidebar, so the marker has to reach them there. The socket is consulted only inside herdr (`HERDR_ENV`), and any failure means no marker — liveness is a hint, never a reason for `list` to fail or hang.
- The edit modal carries a **`[ Release ]` button**, shown only while the item is claimed, sitting between Cancel and Delete in both the button row and the tab ring. It needs no confirmation step (unlike Delete): releasing is reversible — re-claiming restores the link. The sidebar releases with human authority.

## 6. Concurrency ([#8](https://github.com/Adroz/herdr-feedr/issues/8), map fog)

You and agents edit the same file at the same time; that's the point. How nothing gets lost:

- Every sidebar action and every CLI command **reads the file fresh, applies its one change, and writes the whole file back atomically** (temp file + rename — the file on disk is never half-written). Interleaved small edits — you ticking a box while an agent claims an item — all land.
- The sidebar re-reads on every file change, so it always shows current state.
- Every read-modify-write holds an advisory `flock` on a sibling `feed.md.lock`, so concurrent CLI and sidebar writers never lose an update.
- The one loseable case: a whole-file `$EDITOR` session held open while something else writes — the editor's save wins over anything written since the buffer opened. Accepted for v1; keep editor sessions short. Everyday edits go through the safe path.

## 7. Agent status plumbing ([#3](https://github.com/Adroz/herdr-feedr/issues/3))

From the herdr socket API (`~/.config/herdr/herdr.sock`, NDJSON): `pane.list`/`agent.list` for agent kind, status, and `agent_session` UUID; `events.subscribe` for live status changes; `agent.focus` for navigation. Pushed frames are `{"event": …, "data": {"type": …}}`; `layout.updated` (tab id + panes) drives auto-close, and `server.stop` quits herdr when the sidebar was the last pane. Task-plan transcript replay (TaskCreate/TaskUpdate) is **not needed in v1** — agents surface their work by writing to the feed via the CLI; the socket supplies only statuses, the join key, and focus.

## 8. Implementation ([#9](https://github.com/Adroz/herdr-feedr/issues/9))

Rust. One static binary: ratatui sidebar + `feedr` CLI. Packaging copies the plugin-ecosystem conventions (`herdr-plugin.toml` manifest, fetch-or-build script, platform matrix) with repo topics carrying `todo`/`task` keywords for registry search. Theming: none in v1 — follow the terminal palette.

## 9. Out of scope / future

- Herdr core changes.
- Per-project feed scoping/sectioning (future direction; v1 is global-only).
- Sidebar-initiated "send item to agent" (spawn) — v2 candidate; v1 links are created by claims.
- Drag-reorder in the TUI (reorder via `$EDITOR`).
- Task-plan transcript mirroring.
