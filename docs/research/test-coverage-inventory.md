# Research: feedr behaviour inventory vs. test coverage

Ticket: [#16](https://github.com/Adroz/herdr-feedr/issues/16) (map [#14](https://github.com/Adroz/herdr-feedr/issues/14), feeds [#19](https://github.com/Adroz/herdr-feedr/issues/19)).
Snapshot: `48b7d8a` (main), 2026-10-28.

Primary sources: the code under `src/`, `tests/`, `scripts/`, `herdr-plugin.toml`; `docs/SPEC.md`;
`docs/superpowers/specs/2026-09-16-category-field-design.md`; the vendored
`tui-textarea-2 0.13.2` source (for cursor/selection semantics). Every claim below cites a file.

## 1. Baseline

`cargo test` is green: **342 tests** — 306 unit (inline `#[cfg(test)]`), 33 in `tests/cli.rs`,
3 in `tests/skill.rs`. 0 ignored, 0 failing.

Unit tests by module (approx): `feed::ops` 40, `tui::modal` 60, `tui::view` 40, `tui::app` 50,
`tui::dock` 30, `tui::socket` 16, `skill` 11, `identity` 9, `feed::parse` 8, `tui::theme` 8,
`tui::input` 8, `cli` 6, `tui` (mod) 6, `feed::write` 5, `clipboard` 3, `config` 2, `watch` 2, `feed` 2.

Existing test infrastructure (more than the map assumed):

| Infra | Where | Notes |
|---|---|---|
| Temp feed files | `tempfile` everywhere; `app_on_disk()` in `src/tui/app.rs` tests; `seed()` in `tests/cli.rs` | Mature. |
| TestBackend render helpers | `src/tui/test_util.rs` — `render_to_strings`, `render_buffer` (per-cell style), `render_cursor` | Assertion-style, not snapshots. No `insta`. |
| Fake herdr (trait) | `FakeHerdr` in `src/tui/socket.rs:415`; `FakeRunner` in `src/tui/dock.rs:676`; `FakeHerdr`/`FakeEnv` in `src/identity.rs:115-130` | Three separate fakes, not shared. |
| Fake herdr socket (real UDS) | ad-hoc `UnixListener` servers inside `src/tui/socket.rs` tests (lines ~573, 777, 824) | Not a reusable fixture; not usable from `tests/`. |
| Fake `herdr` binary | `tests/cli.rs` auto-dock tests write a shell script and set `HERDR_BIN_PATH` | Only used for auto-dock. |
| Hermetic env | `feedr_bare_env()` in `tests/cli.rs:436` strips `FEEDR_AGENT`/`HERDR_ENV`/`CLAUDE_CODE_SESSION_ID` | Only identity tests use it; `feedr()` inherits the dev's env (see §5). |
| Injected clipboard sink | `App::clipboard_copy: fn(&str)->Result<()>` (`src/tui/app.rs:172`) | Real `pbcopy` never exercised. |

## 2. Coverage map

Legend: **C** covered · **P** partial · **N** none. Test names abbreviated (module prefix dropped).

### 2.1 Feed file — parse / write / ops (`src/feed/*`)

| Behaviour | Cov | Tests |
|---|---|---|
| Parse items, `#`/`##` headings, 2-space bodies | C | `parses_items_headings_and_bodies` |
| Blank-line rules (raw vs interior body blank) | C | `blank_lines_are_raw`, `blank_before_any_body_line_is_raw`, `blank_lines_inside_body_stay_in_body`, `trailing_blank_after_body_is_raw`, `body_with_blank_line_round_trips` |
| Trailing `@agent()`/`@done()` stripped in any order; duplicates stay in title | C | `strips_both_tokens_regardless_of_order`, `duplicate_tokens_stay_in_title` |
| Malformed rightmost token shields the tail (`parse.rs:101`) | P | only via duplicates; no `@agent(nocolon)` case |
| Unknown state char → raw line | C | `unknown_state_char_becomes_raw` |
| `- [X]` (capital X, valid GFM) | **N** | `State::from_char` (`feed/mod.rs:12`) accepts only `x`; a hand-edited `[X]` silently becomes a raw line — invisible in sidebar, unaddressable by CLI |
| CRLF input / no trailing newline / tabs as indent | N | `lines()` strips `\r`, render emits `\n`: CRLF files are silently rewritten to LF on first write |
| Render round-trip lossless for canonical input | C | `round_trip_is_lossless`, `empty_title_tokenless_item_round_trips` |
| Atomic save (temp+rename), keeps permissions | C | `save_atomic_writes_file`, `save_atomic_overwrites_and_preserves_permissions` |
| Zones (Human / Agent / Archive) | C | `zones_follow_headings` |
| `find`: case-insensitive substring, active only, ambiguity error | C | `find_matches_substring_case_insensitive_active_only` |
| `find` when one title is a substring of another ("Fix auth" vs "Fix auth redirect") | **N** | No exact-match preference (`ops.rs:56-85`): the shorter item is **permanently unaddressable** from the CLI |
| claim / review / unclaim / authority | C | `claim_sets_state_and_agent`, `review_sets_question_state`, `agent_cannot_close_human_item`, `agent_can_close_agent_item_and_human_can_close_anything`, `an_agent_releases_its_own_claim`, `an_agent_cannot_release_another_agents_claim`, `releasing_an_unclaimed_item_is_an_error`, `a_handed_back_item_keeps_its_tag_as_a_record`, `the_human_releases_a_claim…` |
| add (human / agent / in-section, body blank-trim) | C | `add_mine_*`, `add_agent_appends_to_agent_section_creating_it`, `add_in_section_*`, `add_keeps_interior_blank_body_line(s)` |
| ensure_section / move_to_section / reserved names | C | `ensure_section_*` (4), `move_to_*` (6) |
| edit / remove / notes | C | `edit_replaces_title_and_body_keeping_tokens`, `remove_deletes_item_with_body_only`, `a_note_*`, `notes_append_after_existing_body_lines_in_order`, `an_empty_note_list_leaves_the_document_untouched` |
| section_names / item_section | C | `section_names_*`, `item_section_names_the_enclosing_level2_heading` |
| Sweep: archive human, delete agent, mirror sections, stamp date, idempotent | C | `sweep_*` (7) |
| Concurrent writers (two processes read-modify-write) | **N** | No lock anywhere (`grep flock` → nothing). Rename prevents torn files but **not lost updates**; SPEC §6 ("interleaved small edits all land") is only true when writes don't overlap |

### 2.2 CLI (`src/cli.rs`, `tests/cli.rs`)

| Behaviour | Cov | Tests |
|---|---|---|
| Feed path precedence `--file` > `FEEDR_FEED` > config `feed_path` > default | C | `precedence_cli_env_config_default`, `feedr_feed_env_selects_feed` |
| `feed_path = "~/..."` in config | N | no tilde expansion (`config.rs:74-88`); path is taken literally |
| `list` (states, sections, hides archive) | C | `list_shows_active_items_with_states`, `list_shows_sections_and_hides_archive` |
| `list`/`show` `(live)` marker; socket only inside `HERDR_ENV`; failure = no marker | P | unit only: `a_confirmed_session_is_marked_live`, `a_socket_failure_is_silently_no_liveness`, `outside_herdr_no_socket_call_is_made`, `inside_herdr_the_live_set_comes_from_the_socket`. No end-to-end against a fake socket |
| `show` prints title + body | C | `show_prints_title_and_body` |
| `show`/`claim` not-found and ambiguous error text at CLI level | P | ops-level only |
| `claim` (flag / env identity, malformed ref, unresolvable leaves feed untouched) | C | `claim_then_review_then_human_done`, `claim_rejects_a_malformed_agent_ref`, `claim_without_the_agent_flag_resolves_identity_from_the_environment`, `an_unresolvable_claim_fails_and_leaves_the_feed_untouched` |
| `unclaim` (own / other's / `--as-human` / unclaimed) | C | `an_agent_unclaims_its_own_item`, `an_agent_cannot_unclaim_someone_elses_item`, `the_human_unclaims_anything`, `unclaiming_an_unclaimed_item_fails` |
| `whoami` | C | `whoami_prints_the_resolved_ref_and_its_source`, `whoami_fails_loudly_when_no_source_resolves` |
| `add` (`--agent-owned`, `--section`, reserved, `--body`) | C | `add_and_sweep`, `add_with_section_*`, `add_with_missing_section_creates_it`, `add_with_reserved_section_fails` |
| `add --agent-owned --section` conflict | N | clap `conflicts_with` — trivial, skip |
| `review` requires `--note`; note written in same save | C | `review_requires_a_note`, `review_writes_the_state_and_the_evidence_note_together` |
| `done` authority, `--note` repeatable/optional | C | `claim_then_review_then_human_done`, `notes_are_repeatable`, `done_takes_an_optional_note` |
| `sweep` | C | `add_and_sweep` |
| Unreadable feed errors instead of clobbering | C | `unreadable_feed_errors_instead_of_clobbering` |
| Missing feed → empty doc, first write creates file (CLI) | P | covered in TUI (`create_modal_save_creates_feed_file_on_first_run`), not via CLI |
| `skill install` / `--refresh-only` / `status` | C | `skill_install_*`, `skill_status_*` + 11 `skill::tests` |
| `skill install` copy fallback when symlink fails | N | `skill.rs:92` fallback branch untested |
| `sidebar` wired; `sidebar` outside a tty is a clean error | P | `sidebar_subcommand_is_wired` (help text only); tty bail (`tui/mod.rs:46`) untested |
| `sidebar --dock` | P | `dock()` unit-tested in `dock.rs`; CLI wiring not |
| `auto-dock-hook` (disabled no-op, needs tab id, plugin-pane-open, fallback) | C | `auto_dock_hook_*` (5) |
| `auto-dock-hook --force` (used by reload-feedr.sh) | N | |
| `fit-sidebar --pane` | P | `fit_width` via `auto_dock_fits_width_through_the_layout_api`; subcommand wiring none |
| Skill doc ↔ clap surface ↔ versions in sync | C | `every_command_the_skill_documents_parses`, `every_subcommand_is_documented_in_the_skill`, `the_skill_stamp_matches_the_crate_and_the_plugin_manifest` |

### 2.3 Identity & config (`src/identity.rs`, `src/config.rs`)

| Behaviour | Cov | Tests |
|---|---|---|
| Resolution order flag > `FEEDR_AGENT` > `herdr pane current` (HERDR_ENV) > `CLAUDE_CODE_SESSION_ID` | C | 9 `identity::tests` |
| Malformed flag/env is an error, not a fall-through | C | `a_malformed_*` |
| Sidebar config defaults, parse, invalid `side` → left | C | `sidebar_config_defaults_and_parse` |
| Malformed `config.toml` → warning + defaults | N | `config.rs:46-52` |
| Nonsensical `width`/`max_width` (≤0, 0, >1) | N | no validation at all |

### 2.4 TUI — main list (`app.rs`, `input.rs`, `view.rs`)

| Behaviour | Cov | Tests |
|---|---|---|
| Rows: sections, items, `@agent` sub-lines, `+` row; empty sections hidden | C | `rows_flatten_sections_items_agent_lines_and_add`, `empty_sections_render_no_header` |
| Checkbox click advances `[ ]→[~]→[x]→[ ]`, `[?]`→accept; never produces `[?]` | C | `advance_cycles_and_review_click_accepts`, `advance_never_produces_review_state`, `checkbox_click_vs_title_click` |
| Title click → edit modal; section header click → create-in-section | C | `checkbox_click_vs_title_click`, `section_header_click_opens_create_in_that_section` |
| Agent sub-line click: focus live pane / resume tab / degrade | C | `agent_click_*` (4) |
| Stale key (title+state changed on disk) drops action with message | C | `stale_key_drops_action_with_message`, `relocate_*` |
| Edit modal open while the item's state changes on disk (agent claims it) | **N** | `ItemKey` = title+state captured at open (`app.rs:16`); Save then hits "item changed on disk — edit dropped" and **the user's typed edits are lost** (modal closes, `save_modal` returns `Modal::None`) |
| Missing-file vs first-run handling in `with_feed` | C | `action_on_deleted_feed_drops_and_does_not_create_empty_file`, `with_feed_proceeds_on_missing_file_…`, `dropped_action_does_not_rewrite_file` |
| Sweep button | C | `sweep_action_archives_and_reports`, `toolbar_clicks` |
| Keys `q a e f ↑ ↓`; scroll wheel; scroll clamps | C | `keys_quit_add_editor_scroll`, `f_key_opens_file_view`, `scroll_*`, `list_scrolls` |
| Keys ignore modifiers (`Ctrl+A` = `a`, `Ctrl+E` = `e`) | N | `input.rs:10-16` — minor, document rather than test |
| Collapse chevron / rail (counts, review & blocked indicators) | C | `status_row_chevron_click_toggles_collapse`, `collapsed_rail_*` (7), `collapse_pane_*`, `expand_pane_*` |
| Toolbar text, Done(n) row, status truncation, ellipsis by display width | C | `toolbar_text_matches_round4_spec`, `renders_toolbar_list_done_and_status_rows`, `long_status_message_truncates_before_chevron`, `ellipsize_*` |
| Live agent glyph; hidden when socket absent | C | `a_live_agent_with_unknown_status_still_shows_a_live_marker`, `agent_glyph_hidden_when_socket_absent` |
| File watcher → reload; coalesced events | C | `watcher_reports_atomic_save`, `watcher_ignores_sibling_files`, `drain_events_coalesces_…` |
| Reload while a modal is open doesn't disturb the modal | N | |
| `$EDITOR` round-trip (suspend, run, re-init mouse/title) | P | `spawn_editor_runs_the_command_with_args`, `reinit_terminal_commands_…`, `open_editor_without_editor_configured_reports`; event-loop suspend/resume not tested |
| Pane zoom on modal open/close | C | `*_zooms_pane_*`, `modal_zoom_*` |

### 2.5 TUI — modals (`modal.rs`, `view.rs`)

| Behaviour | Cov | Tests |
|---|---|---|
| Create/edit layout, buttons, tab ring, Release button only when claimed | C | `edit_layout_*`, `tab_reaches_save_cancel_and_delete…`, `back_tab_cycles_backwards…`, `a_claimed_item_opens_with_a_release_button`, `the_release_button_does_not_overlap_delete`, … |
| Category dropdown (filter, highlight, accept, clipping, resize) | C | `typing_filters_*`, `down_enters_dropdown_enter_accepts`, `down_never_highlights_a_clipped_off_row`, `resized_enter_never_accepts_…`, `click_on_suggestion_accepts_it`, … |
| Save rules: empty title, reserved category, move on category change, byte-stable when unchanged | C | `create_modal_empty_title_rejected`, `reserved_category_rejected_with_status`, `edit_changing_category_*`, `edit_keeping_category_does_not_move_and_stays_byte_stable` |
| Delete with confirm; Release no confirm | C | `delete_requires_confirm`, `clicking_delete_button_opens_confirm_delete`, `the_modal_release_button_clears_the_tag_on_disk` |
| Esc layering: selection → dropdown → modal | C | `esc_cancels_selection_before_dropdown_before_modal_on_category`, `esc_cancels_body_selection_before_cancelling_modal`, `esc_closes_dropdown_first_then_cancels` |
| Done / File viewers (scroll saturates, `e` from file view) | C | `*_view_scroll_down_saturates_past_u16_max`, `done_view_*`, `file_view_shows_raw_feed_and_e_requests_editor` |
| Click-to-position cursor (single line, past-end clamp, border click) | C | `clicking_body_text_places_cursor_at_the_clicked_character`, `clicking_past_end_of_body_text_clamps_cursor_to_line_end`, `clicking_body_border_moves_focus_but_leaves_cursor_unchanged` |
| Body soft-wrap; cursor cell stays inside field | C | `the_body_field_soft_wraps_…`, `reversed_cursor_cell_stays_inside_body_…`, `a_long_body_line_renders_across_several_wrapped_rows` |
| Narrow / short panes don't panic | C | `edit_modal_survives_narrow_pane_render_without_panicking`, `short_pane_collapses_category_keeps_title_usable` |

### 2.6 Text selection & clipboard — known-fragile

| Behaviour | Cov | Tests / finding |
|---|---|---|
| Shift+arrow / Alt(+Shift)+arrow word selection | C | `shift_right_three_times_selects_three_chars_in_body`, `alt_arrows_jump_by_word_shift_extends_selection`, `ctrl_right_jumps_*` |
| Mouse drag, single line, unscrolled | C | `dragging_in_body_selects_the_dragged_span` |
| Down-click clears selection; Tab drops stale selection | C | `down_click_clears_an_existing_selection`, `tab_into_field_drops_stale_selection` |
| **Mouse drag / click across multiple lines** | **N** | — |
| **Click / drag on a soft-wrapped continuation row** | **N — likely bug** | `jump_cursor_to_click` (`modal.rs:676-681`) sends `CursorMove::Jump(y - inner.y, x - inner.x)`. In tui-textarea-2 0.13.2, `Jump` takes a **logical (data) row and char column** (`cursor.rs:437-440`: `DataCursor(row, col).to_screen_cursor`). With `WrapMode::Word` on the body, screen row ≠ logical row, so a click on the 2nd visual row of a wrapped line lands on the next logical line. |
| **Click / drag in a vertically scrolled body** | **N — likely bug** | Same function ignores the textarea's viewport offset (the crate has no screen-to-data public API: `CursorMove` has no screen-relative variant). Once the body scrolls, clicks map to rows above the visible ones. Code comment acknowledges only the *horizontal* case. |
| **Wide chars (CJK, emoji) in click/drag** | **N — likely bug** | `x - inner.x` is a cell offset but `Jump`'s col is a char index (`cursor.rs:439` counts `chars()`); clicks after a wide char land too far right. Sidebar list ellipsis is width-aware (`ellipsize_budgets_by_display_width_not_char_count`) — the modal isn't. |
| Ctrl+C copies only with a non-empty selection | C | `ctrl_c_sets_pending_clipboard_only_when_something_is_selected`, `no_selection_ctrl_c_never_touches_the_sink` |
| Ctrl+X cuts + pending clipboard | C | `ctrl_x_cuts_selection_and_sets_pending_clipboard` |
| Ctrl+V pastes **internal** yank only (not system clipboard) | C | `ctrl_v_pastes_the_internal_yank_buffer`, `native_ctrl_v_binding_is_page_down_scroll_not_paste` |
| Drain to injected sink; failure → status msg | C | `copy_step_drains_…`, `copy_step_leaves_status_untouched_when_the_sink_succeeds` |
| Success gives **no feedback** | — | by design per the test above; worth revisiting under #18 |
| Platform tool choice (pbcopy / wl-copy / xclip) | C | 3 `clipboard::tests` |
| Real `pbcopy` ↔ `pbpaste` round-trip, non-ASCII, large text | **N** | never spawned in tests (deliberate) |
| Terminal paste (Cmd+V) | **N** | No `EnableBracketedPaste` / `Event::Paste` anywhere in `src/`: a pasted multi-line string arrives as keystrokes — in the Title field the first `\n` is an Enter that moves focus to Body (`modal.rs:841`), so the rest of the paste lands in the body; in Category an Enter may accept a dropdown suggestion |
| Cmd+C | — | never reaches the app (terminal/herdr owns Cmd); subject of #15 |

### 2.7 herdr integration (`dock.rs`, `socket.rs`, `watch.rs`)

| Behaviour | Cov | Tests |
|---|---|---|
| Dock: idempotent focus, plugin-pane open, swap-walk, resize, clamp to `max_width`, fallbacks, orphan cleanup | C | `dock_*` (11) |
| Auto-dock per tab; noop if tab has sidebar | C | `auto_dock_*` (5) |
| plan_fit sizing rules | C | `plan_fit_*` (5), `target_width_is_the_share_capped_at_max_width` |
| Socket NDJSON: request/response id match, timeouts, absent socket, agent parsing, status mapping | C | `socket::tests` (16) |
| Event subscription: resync, self-heal on stall, per-pane only | C | `subscribe_loop_*`, `subscribe_never_asks_globally_…` |
| Resume tab per agent kind | C | `resume_args_per_kind`, `open_resume_tab_composes_…`, `live_resume_flow_…` |

### 2.8 Scripts & manifest

| Behaviour | Cov | Notes |
|---|---|---|
| `build.sh`: cargo missing message; build; `skill install --refresh-only` | N | |
| `open-feedr.sh`: find/focus or open + swap-walk + resize + clamp | **N** | Re-implements `dock()` in bash with **hardcoded** `dock_side=left`, `0.18`, `46` (`scripts/open-feedr.sh`) — ignores `config.toml`'s `side`/`width`/`max_width`, unlike `feedr sidebar --dock`. Drift risk with no test either side. |
| `toggle-feedr.sh`: close if open else open | N | |
| `on-tab-created.sh`: tab id from `HERDR_TAB_ID` or sed over `HERDR_PLUGIN_EVENT_JSON` | N | sed regex breaks on whitespace in the JSON (`"tab_id": "…"`) |
| `reload-feedr.sh`: in-place replace, fallback re-dock, `--tab`, `--dry-run`, view restore, exit code | N | Most complex script; only manual testing. |
| Both scripts' and `dock()`'s "find sidebar" pick the **first** marker pane in `pane list` | ? | per-tab correctness depends on whether `herdr pane list` is tab-scoped — unverified here |
| Manifest version = crate = skill stamp | C | `the_skill_stamp_matches_the_crate_and_the_plugin_manifest` |

## 3. Upcoming features that need tests (from map #14 decisions)

### 3.1 Today marker (`@today`)

Parser/writer change: `split_trailing_token` (`parse.rs:97-120`) knows only `@agent(`/`@done(`; `@today`
has no parens, so today a hand-typed `@today` would stay in the title. Tests to write with the feature:

1. Parse `@today` in every order with `@agent(..)`/`@done(..)`; duplicate `@today` stays in title (mirror `duplicate_tokens_stay_in_title`); `@todayish` / `@today(x)` not mistaken.
2. Canonical render order + round-trip byte-stability; item with only `@today` and empty title still parses as an item (cf. `empty_title_tokenless_item_round_trips`).
3. Cap = 3 over `[ ]`/`[~]`/`[?]` across all active zones incl. `## Agent`; 4th mark refused with status, file untouched.
4. `[x]` strips `@today` on **every** path to Done: sidebar advance `[~]→[x]`, accept `[?]→[x]`, `feedr done`, `feedr done --as-human`. Reopen `[x]→[ ]` does not restore it. Sweep never sees `@today`.
5. `claim`/`unclaim`/`review` preserve `@today` (state changes within the counted set don't touch the cap).
6. Hand-edited file with >3: renders all + "4/3" warning; no write path rewrites it (any action that saves must not drop extras).
7. Agents cannot set/unset: no CLI flag mutates it; `feedr list` flags it (output format test).
8. View: virtual Today section on top, items shown only there, dim category suffix; click targets on Today rows map to the right `ItemKey` (row → item mapping currently assumes rows mirror file order, `input.rs:57-75`).
9. Edit-modal toggle: toggling Today is a save that must not trigger a section move; stale-key interaction (toggle while an agent claims → see §2.4 lost-edit gap).
10. Skill: `every_command_the_skill_documents_parses` will catch new syntax; add a skill line about preferring Today items.

### 3.2 Modal click-outside-to-close

Today `edit_click` returns `None` outside targets → `Continue` (`modal.rs:907`); Done/File/ConfirmDelete ignore the mouse entirely. Tests:

1. Read-only modals (Done, File) close on click outside their panel; click inside does not.
2. Edit modal, **clean**: outside click = Esc (closes, no write, unzooms).
3. Edit modal, **dirty**: outside click ignored, status "unsaved — Esc to discard, Ctrl+S to save"; nothing written. Needs a dirty predicate (title/body/category vs original) — test it directly, including "edited then reverted = clean".
4. Open category dropdown: first outside click closes the dropdown only.
5. ConfirmDelete: decide and pin (outside = "n"?).
6. Drag that *starts* inside a field and *ends* outside must not count as an outside click (Down vs Up semantics).
7. Geometry: "outside" in a zoomed pane is the dimmed backdrop; test at narrow/short sizes where the panel nearly fills the area.

## 4. Proposed test list, ranked by risk

Risk = (likelihood it is broken or will break) × (cost: data loss > wrong state > UX). Skips trivial passthroughs (clap conflicts, theme colours, getters).

| # | Test | Why / risk | Layer |
|---|---|---|---|
| 1 | Click and drag on a **soft-wrapped** body row and in a **scrolled** body land on the character under the mouse | Likely live bug (§2.6); core of the "selection doesn't work" complaint | `modal.rs` unit with real `TextArea` + rendered viewport |
| 2 | Edit modal Save after the item's state changed on disk (agent claim) **keeps the user's edits** (or at minimum reopens with them) | Silent loss of typed text | `app.rs` with temp feed |
| 3 | `find` prefers an exact (case-insensitive) title match over substring ambiguity | Shorter-title item permanently unaddressable from CLI/skill | `ops.rs` + `tests/cli.rs` |
| 4 | Concurrent CLI writers: N parallel `feedr claim`/`add` processes on one feed — every change lands | No lock; SPEC §6 promises it. Expect failure → informs whether to add `flock` | `tests/cli.rs` (spawn threads) |
| 5 | `@today` parse/render/cap/strip-on-done suite (§3.1 items 1-6) | New file-format token: highest-blast-radius change upcoming | `parse.rs`, `write.rs`, `ops.rs`, `app.rs` |
| 6 | Multi-line drag selection (down + across lines), then Ctrl+C yields exactly the selected text incl. `\n` | Fragile area, untested beyond one line | `modal.rs` + `app.rs` sink |
| 7 | Wide-char (CJK/emoji) click, drag and cursor render in title/body | Width vs char-index mismatch | `modal.rs`, `view.rs` (`render_cursor`) |
| 8 | Click-outside suite (§3.2) incl. dirty predicate | New behaviour with a data-loss edge (dirty discard) | `modal.rs`, `app.rs` |
| 9 | Hand-edit robustness: `- [X]`, CRLF, tab indent, no trailing newline → decide & pin behaviour (no silent item loss) | Users edit via `$EDITOR`; `[X]` vanishes today | `parse.rs`, `write.rs` |
| 10 | Terminal paste: multi-line paste into Title/Body (needs bracketed paste) | Paste scatters across fields | `tui/mod.rs` + `modal.rs` |
| 11 | CLI `list`/`show` `(live)` end-to-end against a fake herdr UDS server, incl. a stalled socket not hanging `list` | Integration point; agents depend on it | `tests/cli.rs` + shared fake socket |
| 12 | `open-feedr.sh` / `toggle-feedr.sh` / `reload-feedr.sh` / `on-tab-created.sh` against a fake `herdr` binary (record argv) | Zero coverage, user-facing entry points; open-feedr ignores config | `tests/scripts.rs` via `HERDR_BIN_PATH` |
| 13 | Gated real clipboard e2e: `clipboard::copy` → `pbpaste` (macOS) / `wl-paste` round-trip, non-ASCII + 100 KB | Only path never executed in CI | `#[ignore]` or `FEEDR_E2E_CLIPBOARD=1` |
| 14 | Feed reload (watcher event) while edit modal is open leaves modal state intact | Plausible regression as reload logic grows | `app.rs` |
| 15 | Malformed `config.toml` → defaults + warning; bad `width`/`max_width` values clamp or error | Config errors currently silent/unchecked | `config.rs` |
| 16 | `skill install` copy fallback when symlink fails; `auto-dock-hook --force`; `fit-sidebar` wiring | Small untested branches on install/reload paths | `skill.rs`, `tests/cli.rs` |
| 17 | `feedr sidebar` outside a tty exits non-zero with message (not a panic) | Stated acceptance criterion, untested | `tests/cli.rs` |
| 18 | Render snapshots of main list, rail, each modal at 3 sizes | Catches layout drift the assertion tests miss | `insta` + `TestBackend` |

## 5. Test-infra gaps

1. **Render snapshots** — `test_util.rs` gives strings/buffers but every assertion is hand-written. Add `insta` (dev-dep) with a `snapshot(app, w, h)` helper over `render_to_strings`; keep style checks on `render_buffer`.
2. **Shared fake herdr socket** — the `UnixListener` servers are inlined per test in `socket.rs`. Extract one scriptable fake (canned responses by method, optional stall, event stream) usable from unit tests *and* `tests/cli.rs` (point the child at it with `HERDR_SOCKET_PATH`, `socket.rs:159`).
3. **One fake herdr** — three fakes (`socket::FakeHerdr`, `dock::FakeRunner`, `identity` `FakeHerdr`) cover overlapping surfaces; a single recorded-call fake would make script/CLI/TUI tests consistent.
4. **Fake `herdr` binary for scripts** — generalise the auto-dock shell stub in `tests/cli.rs` into a fixture that logs argv and replays canned `pane list`/`plugin pane open` JSON, so the four scripts get integration tests.
5. **Hermetic CLI env** — `tests/cli.rs::feedr()` inherits the developer's `HERDR_ENV`/`CLAUDE_CODE_SESSION_ID`/`HOME`; inside herdr, `list`/`show` tests hit the real socket. Make `feedr()` start from `feedr_bare_env` semantics plus a temp `HOME`.
6. **Gated real clipboard e2e** — `pbcopy`↔`pbpaste` behind an env flag or `#[ignore]`, macOS-only `cfg`; Linux variant needs a display, so skip in CI.
7. **Textarea viewport access** — tests for #1/#7 need to drive the body's viewport (scroll) deterministically; may require rendering through `TestBackend` before stepping events, since tui-textarea updates its viewport during render.
8. **Event-loop seam** — `tui::event_loop` (editor suspend/resume, coalesced reloads) is only testable piecemeal; a function taking an event iterator + `TestBackend` would let end-to-end TUI scenarios run headless.
9. **Concurrency harness** — a helper that spawns N `feedr` children against one temp feed and asserts all mutations are present.

## 6. Notes for #19 (test plan grilling)

- Coverage of pure logic (ops, parse, dock planning, socket protocol) is already strong; the gaps cluster at **seams**: screen↔text coordinate mapping, disk-vs-modal staleness, process boundaries (clipboard, scripts, concurrent CLI), and hand-edited input.
- Three findings are likely bugs rather than missing tests and could be fixed alongside: wrapped/scrolled click mapping (#1), lost edits on stale save (#2), substring-only `find` (#3). Concurrent lost-update (#4) contradicts SPEC §6's wording.
- `open-feedr.sh` duplicating `dock()` with hardcoded values is a design question (replace with `feedr sidebar --dock`?) more than a test question.
