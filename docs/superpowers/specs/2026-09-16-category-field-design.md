# Category field for tasks — design

Add a category (subject/topic) to tasks: topmost field in the create/edit modal with
autocomplete over past categories, free text for new ones; the main list groups
visually by category.

Decided 2026-09-16. Approach: **categories are the existing `##` sections** — no file
format change. A per-item `@topic()` token was rejected: it would break the
"top-to-bottom is priority order" principle (spec §1), require virtual grouping in the
view, and rework sweep mirroring.

## 1. File format

Unchanged. A category is a `##` heading in the human zone. Uncategorized items are the
items under `# Feed` before the first `##` heading. `## Agent` and `# Done` are
reserved, and so is `Feed` (the archive's mirror name for uncategorized items); none
can be a category (case-insensitive).

## 2. Modal

The create/edit modal gains a **Category** field, topmost. Tab order:
Category → Title → Body → Save → Cancel → (Delete, edit mode only).

Single-line input with a suggestion dropdown:

- **Suggestions** = active human `##` section names + the `# Done` archive's mirrored
  `##` section names ("categories used in the past"), case-insensitively deduplicated,
  excluding Agent and Done.
- Filtered by case-insensitive substring match as the user types.
- **Down** moves focus into the dropdown, **Up/Down** move the highlight, **Enter**
  accepts the highlighted suggestion. **Enter** with the dropdown closed advances
  focus to Title (the field is single-line; no newline is inserted). **Esc** closes
  the dropdown; a second Esc cancels the modal. Text not matching any suggestion is a new category, used verbatim
  (trimmed).
- **Create mode, empty field**: current behavior — item lands at the end of the
  uncategorized region (end of first human section).
- **Create mode, non-empty**: item appends to the end of that section; if the section
  doesn't exist it is created (see §3 placement).
- **Edit mode**: field prefilled with the item's current section name (empty if
  uncategorized). Changed on save → the item moves to the end of the target section
  with state, body, and `@agent`/`@done` tokens intact. Unchanged → no move (item
  keeps its position within its section).

## 3. Ops layer (`feed/ops.rs`)

New operations, both pure `Document` mutations on the existing
read-fresh → mutate → atomic-rename write path (spec §6):

- `ensure_section(doc, name) -> index` — find the active `## name` heading
  (case-insensitive) or create it at the **end of the human zone**: before `## Agent`
  if present, else before `# Done`, else end of file. Returns the index just past the
  section's last item (insertion point).
- `move_to_section(doc, item_index, name)` — remove the item node (with its body) and
  reinsert it at the end of the named section, creating the section if needed. One
  mutation, so a single atomic write covers the whole move.
- A cleared (empty) Category on edit moves the item to the **true uncategorized
  region** — the end of the items before the first `##` heading — even when that
  region is currently empty (a dedicated `end_of_uncategorized_region` helper; the
  legacy `end_of_first_human_section`, which can land inside the first named section,
  stays for the create-with-empty-Category path only).

Creation and no-move edits reuse the existing `add_in_section` / in-place update
paths.

## 4. Concurrency and safety

- Adds are pure inserts; moves are one read-modify-write keyed by `ItemKey`
  (title + state) against a fresh read. A missed or ambiguous match **drops the action
  with a status message** — never guesses, never deletes. A task can have a move
  refused; it cannot be lost.
- Known edge: two active items sharing title + state make the key ambiguous and the
  sidebar refuses the action. Future fix if it bites: include the section name in
  `ItemKey`. Out of scope here.

## 5. Empty sections

A section whose last item moves out **keeps its heading** — it stays in the file and
the list, and continues to autocomplete. Deleting a category is a `$EDITOR` edit.

## 6. List view

No changes: `##` sections already render as group headers (`Row::Section`), and
grouping is physical in the file, so display order remains file order.

## 7. Out of scope

- ~~CLI `feedr add --section <name>` parity~~ — shipped alongside this feature after
  all: `--section` now creates a missing section and rejects reserved names (see
  plan Task 3).
- `ItemKey` disambiguation by section (see §4).
- Reordering or renaming sections from the sidebar — `$EDITOR` covers it.

## 8. Testing

- **Ops**: `ensure_section` placement (before `## Agent` / before `# Done` / EOF,
  case-insensitive find); `move_to_section` preserves state, body, and tokens; move to
  own section is a no-op; reserved names rejected; heading kept when the section
  empties.
- **Modal**: tab/focus order with the new field; suggestion filtering and
  accept/dismiss; Esc layering (dropdown first, then modal); empty-field create takes
  the current uncategorized path; edit-mode prefill.
- **Round-trip**: saving an edit without changing category leaves the file byte-stable
  (canonical input).
