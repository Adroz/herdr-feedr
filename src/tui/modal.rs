//! Edit/create modal state machine (spec §3 "One modal serves both edit and
//! create"). Kept in its own module — separate from `App` — because the
//! modal's field set (section picker, two `TextArea`s, focus) is a distinct
//! sub-state machine, not app-wide state (Task 7 review: app.rs strains when
//! unrelated state is flattened into it).
//!
//! `step` is the pure key/event → transition function, mirroring
//! `input::translate`'s pure hit-testing: it never touches the filesystem.
//! The two outcomes that require a feed write (`Save`, `Delete`) are handed
//! back to `App::handle_modal_event`, which alone holds `with_feed` access.

use crate::feed::Item;
use crate::tui::app::ItemKey;
use crate::tui::theme;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Padding};
use tui_textarea::{CursorMove, TextArea};

pub enum Modal {
    None,
    Edit(EditModal),
    ConfirmDelete(EditModal),
    /// Read-only view of the `# Done` archive (spec §3).
    DoneView {
        scroll: u16,
    },
    /// Read-only view of the whole feed file; `e` requests `$EDITOR`.
    FileView {
        scroll: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFocus {
    Category,
    Title,
    Body,
    Save,
    Cancel,
    /// Edit mode only — create mode has no item to delete.
    Delete,
}

pub const SAVE_LABEL: &str = "[ Save ]";
pub const CANCEL_LABEL: &str = "[ Cancel ]";
pub const DELETE_LABEL: &str = "[ Delete ]";
const BUTTON_GAP: u16 = 2;

/// Visible dropdown rows are capped; the keyboard highlight is clamped to
/// the same cap so it can never point at an invisible row.
pub const MAX_DROPDOWN_ROWS: usize = 5;

pub struct EditModal {
    /// `Some(key)` = editing an existing item; `None` = creating.
    pub original: Option<ItemKey>,
    /// Edit mode: the section the item was in when the modal opened (None =
    /// uncategorized). Compared with `category_text()` on save to decide
    /// whether the item moves. Always None in create mode.
    pub original_category: Option<String>,
    pub category: TextArea<'static>,
    pub title: TextArea<'static>,
    pub body: TextArea<'static>,
    /// Category suggestions captured at open (`ops::section_names`).
    pub suggestions: Vec<String>,
    /// Keyboard highlight into `filtered()`; None = not in the list.
    pub dropdown: Option<usize>,
    pub focus: EditFocus,
    /// Text yanked by a Ctrl+C/Ctrl+X on the focused field, waiting to be
    /// pushed to the SYSTEM clipboard. `edit_step` stays pure (no process
    /// spawning), so it only records the text here; `App::handle_modal_event`
    /// drains it after every `step` call and does the actual spawn (mirrors
    /// `App`'s `editor_request`/`take_editor_request` one-shot pattern).
    pub pending_clipboard: Option<String>,
}

impl EditModal {
    /// Sidebar-created items are always the human's. Category is the topmost
    /// field (spec 2026-09-16 §2): empty keeps the pre-category behavior
    /// (end of first human section), anything else names a section.
    pub fn create(suggestions: Vec<String>) -> Self {
        let mut m = EditModal {
            original: None,
            original_category: None,
            category: TextArea::default(),
            title: TextArea::default(),
            body: TextArea::default(),
            suggestions,
            dropdown: None,
            focus: EditFocus::Category,
            pending_clipboard: None,
        };
        m.sync_blocks();
        m
    }

    pub fn edit(
        key: ItemKey,
        item: &Item,
        section: Option<String>,
        suggestions: Vec<String>,
    ) -> Self {
        // An already-categorized item opens focused on Title: the category
        // is usually staying put, and Category focus would pop the
        // suggestion dropdown over the Title field for no reason. An
        // uncategorized item keeps Category-first (like create) — assigning
        // one is the likely next step there.
        let focus = if section.is_some() {
            EditFocus::Title
        } else {
            EditFocus::Category
        };
        let mut m = EditModal {
            original: Some(key),
            category: TextArea::new(vec![section.clone().unwrap_or_default()]),
            original_category: section,
            title: TextArea::new(vec![item.title.clone()]),
            body: TextArea::new(item.body.clone()),
            suggestions,
            dropdown: None,
            focus,
            pending_clipboard: None,
        };
        // `TextArea::new` already leaves the cursor (and viewport) at (0,0)
        // — left there deliberately, not overridden to End/Bottom. This
        // used to park all three cursors at the end on the theory that
        // appending is the common edit, but that let tui-textarea scroll
        // its viewport to keep the parked cursor visible on the modal's
        // FIRST frame (rendered at the narrow pre-zoom pane width) — and
        // tui-textarea never scrolls back once the pane later widens. Net
        // effect: opening any item with a long line, or a multi-line body,
        // showed it clipped to its tail even though nothing was focused
        // there yet. Viewport correctness on open beats append convenience:
        // the end is still one Ctrl+E / End-key / click away, and starting
        // at Head also makes click-to-position exact (no hidden scroll
        // offset to account for).
        m.sync_blocks();
        m
    }

    /// Tab order: Category → Title → Body → Save → Cancel → (Delete →) back
    /// to the start. Create mode has no item to delete.
    pub fn cycle_focus(&mut self) {
        let is_edit = self.original.is_some();
        self.focus = match (self.focus, is_edit) {
            (EditFocus::Category, _) => EditFocus::Title,
            (EditFocus::Title, _) => EditFocus::Body,
            (EditFocus::Body, _) => EditFocus::Save,
            (EditFocus::Save, _) => EditFocus::Cancel,
            (EditFocus::Cancel, true) => EditFocus::Delete,
            (EditFocus::Cancel, false) => EditFocus::Category,
            (EditFocus::Delete, _) => EditFocus::Category,
        };
        self.sync_blocks();
    }

    /// Shift+Tab: the exact reverse of `cycle_focus`.
    pub fn cycle_focus_back(&mut self) {
        let is_edit = self.original.is_some();
        self.focus = match (self.focus, is_edit) {
            (EditFocus::Category, true) => EditFocus::Delete,
            (EditFocus::Category, false) => EditFocus::Cancel,
            (EditFocus::Title, _) => EditFocus::Category,
            (EditFocus::Body, _) => EditFocus::Title,
            (EditFocus::Save, _) => EditFocus::Body,
            (EditFocus::Cancel, _) => EditFocus::Save,
            (EditFocus::Delete, _) => EditFocus::Cancel,
        };
        self.sync_blocks();
    }

    /// Standard form behavior: tabbing INTO a text field parks its cursor at
    /// the end of its content. Any stale selection is dropped first —
    /// `move_cursor` extends an active selection, so the jump would
    /// otherwise silently select everything up to the end. Applied on focus
    /// TRANSITIONS only (Tab / Shift+Tab / Enter-advance), never at modal
    /// construction: the modal's first frame can render at the narrow
    /// pre-zoom pane width, and an end-parked cursor there scrolls the
    /// viewport irreversibly (the open-scrolled bug). Mouse clicks position
    /// the cursor explicitly and bypass this.
    pub fn park_focused_field_at_end(&mut self) {
        let ta = match self.focus {
            EditFocus::Category => &mut self.category,
            EditFocus::Title => &mut self.title,
            EditFocus::Body => &mut self.body,
            EditFocus::Save | EditFocus::Cancel | EditFocus::Delete => return,
        };
        ta.cancel_selection();
        ta.move_cursor(CursorMove::Bottom);
        ta.move_cursor(CursorMove::End);
    }

    /// Mark the focused field's border title with `*` so focus is visible,
    /// and set each textarea's own cursor styling to match focus.
    ///
    /// Round-2 feedback item 1 neutralized every field's cursor style and
    /// relied on the real terminal cursor (placed by
    /// `view::draw_edit_modal` via `frame.set_cursor_position`) as the only
    /// visible cursor. That decision is reversed here: the real terminal
    /// cursor was placed from the textarea's LOGICAL cursor column, which
    /// drifts from the drawn text whenever tui-textarea has scrolled its
    /// viewport horizontally (any line longer than the field's width) —
    /// tui-textarea 0.7 exposes no viewport offset to correct by.
    ///
    /// The focused field shows tui-textarea's own rendered cursor cell
    /// instead — it is always viewport-correct, unlike a real terminal
    /// cursor placed from the LOGICAL cursor column (which drifts whenever
    /// the textarea has scrolled horizontally; tui-textarea 0.7 exposes no
    /// viewport offset to correct by). Unfocused fields get an invisible
    /// cursor style so only one cursor shows.
    pub fn sync_blocks(&mut self) {
        let c = if self.focus == EditFocus::Category {
            "Category*"
        } else {
            "Category"
        };
        let t = if self.focus == EditFocus::Title {
            "Title*"
        } else {
            "Title"
        };
        let b = if self.focus == EditFocus::Body {
            "Body*"
        } else {
            "Body"
        };
        self.category.set_block(Block::bordered().title(c));
        self.title.set_block(Block::bordered().title(t));
        self.body.set_block(Block::bordered().title(b));
        // Selection highlight: reapplied here (not just at construction)
        // because `set_category` replaces `self.category` with a brand new
        // `TextArea`, which would otherwise fall back to tui-textarea's own
        // default (light blue) selection style.
        self.category.set_selection_style(theme::selection());
        self.title.set_selection_style(theme::selection());
        self.body.set_selection_style(theme::selection());
        // tui-textarea underlines the cursor's line by default
        // (`cursor_line_style`); in these mostly-single-line fields that
        // reads as "everything is underlined" for no reason — neutralize it.
        self.category.set_cursor_line_style(Style::default());
        self.title.set_cursor_line_style(Style::default());
        self.body.set_cursor_line_style(Style::default());
        let visible = Style::default().add_modifier(Modifier::REVERSED);
        let invisible = Style::default();
        self.category
            .set_cursor_style(if self.focus == EditFocus::Category {
                visible
            } else {
                invisible
            });
        self.title
            .set_cursor_style(if self.focus == EditFocus::Title {
                visible
            } else {
                invisible
            });
        self.body
            .set_cursor_style(if self.focus == EditFocus::Body {
                visible
            } else {
                invisible
            });
    }

    pub fn category_text(&self) -> String {
        self.category.lines().join(" ").trim().to_string()
    }

    pub fn title_text(&self) -> String {
        self.title.lines().join(" ").trim().to_string()
    }

    pub fn body_lines(&self) -> Vec<String> {
        let lines: Vec<String> = self.body.lines().to_vec();
        if lines == vec![String::new()] {
            Vec::new() // an untouched textarea is one empty line, not a body
        } else {
            lines
        }
    }

    /// Suggestions matching the field, case-insensitive substring.
    pub fn filtered(&self) -> Vec<String> {
        let q = self.category_text().to_lowercase();
        self.suggestions
            .iter()
            .filter(|s| s.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    /// Replace the field content with an accepted suggestion. Parking the
    /// cursor at End here (unlike `edit()`, which now parks at start) is
    /// fine: the user just acted on this field directly, so there's no
    /// stale-viewport-on-first-frame risk, and category names are short
    /// enough that End never triggers a horizontal scroll anyway.
    pub fn set_category(&mut self, name: &str) {
        self.category = TextArea::new(vec![name.to_string()]);
        self.category.move_cursor(CursorMove::End);
        self.sync_blocks();
    }

    /// Rows the suggestion dropdown occupies (0 when Category unfocused or
    /// nothing matches) — the single source for both `view::draw_edit_modal`
    /// and `edit_step`'s hit-testing, so drawn and clickable rows can never
    /// drift apart.
    pub fn dropdown_rows(&self) -> u16 {
        if self.focus == EditFocus::Category {
            self.filtered().len().min(MAX_DROPDOWN_ROWS) as u16
        } else {
            0
        }
    }

    /// Accept suggestion `i` from `filtered()` — shared by the keyboard
    /// (Enter on a highlighted row) and mouse (click on a row) accept paths
    /// (B4 review follow-up) so they can never drift apart.
    pub fn accept_suggestion(&mut self, i: usize) {
        if let Some(name) = self.filtered().get(i).cloned() {
            self.set_category(&name);
        }
        self.dropdown = None;
    }
}

/// Rows of the dropdown actually drawn at this terminal size — the Down and
/// Enter arms both clamp against this so the keyboard can neither highlight
/// nor accept a suggestion the user cannot see (the rect may be clipped by
/// `.intersection(term_area)` at degenerate sizes).
fn visible_rows(m: &EditModal, area: Rect) -> usize {
    edit_layout(area, m.original.is_some(), m.dropdown_rows())
        .dropdown_inner
        .height as usize
}

/// Drawn geometry of the edit/create modal — the single source of truth for
/// both where things are rendered (`view::draw_modal`) and where a click
/// lands (`edit_step`'s mouse handling), so the two can never drift apart.
/// Mirrors the main list's `view::draw` / `input::translate` split, except
/// here the geometry itself is a shared pure function rather than a
/// duplicated formula, since the modal's layout is nested (centered rect →
/// bordered inner → vertical stack → button spans).
#[derive(Debug, Clone, Copy)]
pub struct EditLayout {
    pub outer: Rect,
    pub category: Rect,
    /// Suggestion dropdown's OUTER (bordered) box. Zero-height when the
    /// dropdown is closed. Overlays the title/body area; `edit_click` tests
    /// it first so overlap resolves to the list. A click inside this rect
    /// but outside `dropdown_inner` lands on the dropdown's own border —
    /// consumed as a no-op rather than picking a suggestion or falling
    /// through to Title/Body beneath.
    pub dropdown: Rect,
    /// The dropdown's row-bearing area, inside its own border — the single
    /// source both `view::draw_edit_modal` (what it paints rows into) and
    /// `edit_click`/`visible_rows` (what it hit-tests/counts against) derive
    /// from, via `dropdown_inner_of`, so drawn and clickable rows can never
    /// drift apart. Zero-height when the dropdown is closed.
    pub dropdown_inner: Rect,
    pub title: Rect,
    pub body: Rect,
    pub hints: Rect,
    pub save: Rect,
    pub cancel: Rect,
    /// `None` in create mode — nothing to delete yet.
    pub delete: Option<Rect>,
}

/// Percentage-centered rect within `area` (shared by the edit modal, the
/// confirm-delete modal, and the read-only viewers).
pub(crate) fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let [_, mid_v, _] = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .areas(area);
    let [_, mid, _] = Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .areas(mid_v);
    mid
}

/// Pure geometry for the edit/create modal, given the full terminal area.
/// One cell of inner padding (round-3 item 1) — the same padding
/// `view::draw_edit_modal`'s `Block` is drawn with, so the visual panel and
/// this hit-tested geometry can never drift apart.
///
/// Crash fix: the button row lays out Save/Cancel/Delete by walking an
/// x-cursor across their raw label widths + gaps. At the ~30-col width
/// herdr docks a sidebar pane at, that sum (32 cells) can exceed not just
/// the modal panel but the terminal itself — the button `Rect`s must never
/// be allowed past `term_area`'s own right edge, because `view::draw_edit_modal`
/// hands them straight to `Paragraph::render`, which indexes the frame's
/// buffer directly rather than clipping. Every button rect is therefore
/// intersected with `term_area` before being returned: on a wide-enough
/// terminal this is a no-op (the intersection equals the original rect); at
/// a narrow one it clamps (or, for a button that starts entirely past the
/// edge, zeroes) the rect so it can never escape the buffer that's actually
/// being drawn into — the same rect that's later used to hit-test clicks
/// (`edit_click`), so a clipped-away button also becomes unclickable rather
/// than clickable-but-invisible.
pub fn edit_layout(term_area: Rect, is_edit: bool, dropdown_rows: u16) -> EditLayout {
    let outer = centered(term_area, 90, 80);
    let inner = Block::bordered().padding(Padding::uniform(1)).inner(outer);
    // Review follow-up (B1): at inner heights below 9 the constraint solver
    // starves Title (title height 0-2 = invisible text) if Category keeps
    // its full 3-row height. Collapsing Category to 0 rows on short panes
    // keeps Title usable — the Category field is still reachable, it just
    // isn't drawn at this size.
    let category_len = if inner.height < 9 { 0 } else { 3 };
    let [category, title, body, buttons, hints] = Layout::vertical([
        Constraint::Length(category_len),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    // Directly below Category, same width — reads as the field's own box
    // continuing downward. +2 rows for the dropdown's own top/bottom
    // border (zero-height, and so empty/undrawn, when the dropdown is
    // closed). Clamped like the buttons so it can never escape the drawn
    // buffer.
    let dropdown_outer_height = if dropdown_rows == 0 {
        0
    } else {
        dropdown_rows.saturating_add(2)
    };
    let dropdown = Rect::new(
        category.x,
        category.y.saturating_add(category.height),
        category.width,
        dropdown_outer_height,
    )
    .intersection(term_area);
    let dropdown_inner = dropdown_inner_of(dropdown);

    let mut x = buttons.x;
    let save = Rect::new(x, buttons.y, SAVE_LABEL.len() as u16, 1).intersection(term_area);
    x = x
        .saturating_add(SAVE_LABEL.len() as u16)
        .saturating_add(BUTTON_GAP);
    let cancel = Rect::new(x, buttons.y, CANCEL_LABEL.len() as u16, 1).intersection(term_area);
    x = x
        .saturating_add(CANCEL_LABEL.len() as u16)
        .saturating_add(BUTTON_GAP);
    let delete = is_edit
        .then(|| Rect::new(x, buttons.y, DELETE_LABEL.len() as u16, 1).intersection(term_area));

    EditLayout {
        outer,
        category,
        dropdown,
        dropdown_inner,
        title,
        body,
        hints,
        save,
        cancel,
        delete,
    }
}

/// The suggestion dropdown's row-bearing area, inside its own border — the
/// single source of truth `view::draw_edit_modal` (what it paints rows
/// into) and `edit_click`/`visible_rows` (what they hit-test/count against)
/// both derive from, so drawn and clickable rows can never drift apart.
/// `Block::inner` saturates rather than underflowing when `outer` is too
/// small to fit a border (e.g. clipped to zero height when closed), so no
/// separate empty-rect guard is needed here.
pub(crate) fn dropdown_inner_of(outer: Rect) -> Rect {
    Block::bordered().inner(outer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditClickTarget {
    Category,
    Suggestion(u16),
    Title,
    Body,
    Save,
    Cancel,
    Delete,
}

fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// Hit-test a click against the modal's drawn geometry. Pure — no
/// filesystem or `App` access, mirrors `input::translate`'s hit-testing for
/// the main list.
pub fn edit_click(layout: &EditLayout, is_edit: bool, x: u16, y: u16) -> Option<EditClickTarget> {
    if layout.dropdown.height > 0 && rect_contains(layout.dropdown, x, y) {
        // Inside the dropdown's outer (bordered) box: a hit on the
        // row-bearing inner area picks a suggestion; a hit on the border
        // itself (e.g. the top/bottom frame) is consumed as a no-op — it
        // must not pick a suggestion NOR fall through to Category/Title
        // beneath, since the dropdown visually overlays them here.
        if rect_contains(layout.dropdown_inner, x, y) {
            return Some(EditClickTarget::Suggestion(y - layout.dropdown_inner.y));
        }
        return None;
    }
    if rect_contains(layout.category, x, y) {
        return Some(EditClickTarget::Category);
    }
    if rect_contains(layout.title, x, y) {
        return Some(EditClickTarget::Title);
    }
    if rect_contains(layout.body, x, y) {
        return Some(EditClickTarget::Body);
    }
    if rect_contains(layout.save, x, y) {
        return Some(EditClickTarget::Save);
    }
    if rect_contains(layout.cancel, x, y) {
        return Some(EditClickTarget::Cancel);
    }
    if is_edit {
        if let Some(d) = layout.delete {
            if rect_contains(d, x, y) {
                return Some(EditClickTarget::Delete);
            }
        }
    }
    None
}

/// Result of feeding one terminal event to the active modal. `Continue`
/// covers every transition that stays purely in-memory (including
/// cancelling to `Modal::None`); `Save` and `Delete` are the two points
/// where the caller must go through `with_feed` to touch the file.
pub enum ModalStep {
    Continue(Modal),
    Save(EditModal),
    Delete(ItemKey),
    /// The `FileView`'s `e` key: only `App` can turn this into an editor
    /// request (it needs `editor_cmd`/`status_msg`), so hand it back like
    /// `Save`/`Delete`.
    OpenEditor,
}

/// Pure modal transition: `(modal, event, terminal area) -> ModalStep`.
/// Never touches the filesystem — mirrors `input::translate`. `area` is the
/// full terminal area (as drawn by `view::draw`); only `Modal::Edit`'s mouse
/// handling needs it, to re-derive `edit_layout` for hit-testing.
pub fn step(modal: Modal, ev: Event, area: Rect) -> ModalStep {
    // Ignore key releases (Windows terminals emit them).
    if let Event::Key(k) = &ev {
        if k.kind == KeyEventKind::Release {
            return ModalStep::Continue(modal);
        }
    }
    match modal {
        Modal::None => ModalStep::Continue(Modal::None),
        Modal::Edit(m) => edit_step(m, ev, area),
        Modal::ConfirmDelete(m) => match ev {
            Event::Key(k) if k.code == KeyCode::Char('y') => {
                let key = m
                    .original
                    .clone()
                    .expect("delete only offered when editing");
                ModalStep::Delete(key)
            }
            Event::Key(k) if matches!(k.code, KeyCode::Char('n') | KeyCode::Esc) => {
                ModalStep::Continue(Modal::Edit(m))
            }
            _ => ModalStep::Continue(Modal::ConfirmDelete(m)),
        },
        Modal::DoneView { scroll } => match ev {
            Event::Key(k) if matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) => {
                ModalStep::Continue(Modal::None)
            }
            Event::Key(k) if k.code == KeyCode::Down => ModalStep::Continue(Modal::DoneView {
                scroll: scroll.saturating_add(1),
            }),
            Event::Key(k) if k.code == KeyCode::Up => ModalStep::Continue(Modal::DoneView {
                scroll: scroll.saturating_sub(1),
            }),
            _ => ModalStep::Continue(Modal::DoneView { scroll }),
        },
        Modal::FileView { scroll } => match ev {
            Event::Key(k) if matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) => {
                ModalStep::Continue(Modal::None)
            }
            Event::Key(k) if k.code == KeyCode::Char('e') => ModalStep::OpenEditor,
            Event::Key(k) if k.code == KeyCode::Down => ModalStep::Continue(Modal::FileView {
                scroll: scroll.saturating_add(1),
            }),
            Event::Key(k) if k.code == KeyCode::Up => ModalStep::Continue(Modal::FileView {
                scroll: scroll.saturating_sub(1),
            }),
            _ => ModalStep::Continue(Modal::FileView { scroll }),
        },
    }
}

/// Click-to-position: map the click to a cursor location. Coordinates are
/// viewport-relative under the assumption the field isn't horizontally
/// scrolled — with the focused-field cursor fix the common case — and
/// `CursorMove::Jump` clamps past-end coordinates to the line/text end
/// (`tui-textarea` 0.7 `cursor.rs`'s `Jump` arm: row clamps to the last
/// line, col clamps to that line's length via `fit_col`).
///
/// `field_rect` is the field's full bordered `Rect` (as drawn); a click
/// inside the border but outside the text-bearing inner rect (i.e. on the
/// border itself) focuses the field without moving its cursor — there is no
/// clicked character to jump to there.
///
/// Caveat: if the field IS horizontally scrolled (its current line is
/// longer than the field's width), the jump is offset by the hidden
/// scroll — the same unexposed-viewport limitation the cursor-rendering fix
/// works around for the focused field's own cursor cell. This is an
/// acceptable degradation: the click still lands inside the right field,
/// just not necessarily on the exact character under the mouse.
fn jump_cursor_to_click(ta: &mut TextArea<'static>, field_rect: Rect, x: u16, y: u16) {
    let inner = Block::bordered().inner(field_rect);
    if rect_contains(inner, x, y) {
        ta.move_cursor(CursorMove::Jump(y - inner.y, x - inner.x));
    }
}

/// The textarea backing whichever field currently has focus — `None` when
/// focus is on a button (Save/Cancel/Delete), which has no textarea.
/// Shared by the selection-clipboard key arms, the Esc selection-cancel
/// layer, and mouse-drag selection, so they can't drift on which field
/// "focused" means.
fn focused_textarea_mut(m: &mut EditModal) -> Option<&mut TextArea<'static>> {
    match m.focus {
        EditFocus::Category => Some(&mut m.category),
        EditFocus::Title => Some(&mut m.title),
        EditFocus::Body => Some(&mut m.body),
        EditFocus::Save | EditFocus::Cancel | EditFocus::Delete => None,
    }
}

/// Ctrl+C: copy the focused field's selection into tui-textarea's own yank
/// buffer (`TextArea::copy`, native — a no-op without a selection) and
/// report the yanked text so the caller can also push it to the SYSTEM
/// clipboard via `EditModal::pending_clipboard`. Returns `None` — leaving
/// `pending_clipboard` untouched — when there's nothing to copy: no field
/// focused, or no active (non-empty) selection. Gated on `selection_range()`
/// rather than `is_selecting()` so a degenerate zero-length selection
/// (cursor back at its anchor) doesn't falsely report a copy.
fn copy_focused(m: &mut EditModal) -> Option<String> {
    let ta = focused_textarea_mut(m)?;
    let had_selection = ta.selection_range().is_some();
    ta.copy();
    had_selection.then(|| ta.yank_text())
}

/// Ctrl+X: same gating as `copy_focused`, via `TextArea::cut` (native —
/// removes the selected text and moves the cursor to its start).
fn cut_focused(m: &mut EditModal) -> Option<String> {
    let ta = focused_textarea_mut(m)?;
    let had_selection = ta.selection_range().is_some();
    ta.cut();
    had_selection.then(|| ta.yank_text())
}

/// Ctrl+V: paste tui-textarea's own internal yank buffer (`TextArea::paste`)
/// into the focused field — NOT the system clipboard (approved scope: Ctrl+V
/// is internal-yank-only). Explicitly intercepted rather than left to
/// `TextArea::input()`: tui-textarea 0.7 maps native Ctrl+V to
/// `Scrolling::PageDown`, not paste (its paste binding is emacs-style
/// Ctrl+Y) — see `native_ctrl_v_binding_is_page_down_scroll_not_paste`
/// for the pinned source citation.
fn paste_focused(m: &mut EditModal) {
    if let Some(ta) = focused_textarea_mut(m) {
        ta.paste();
    }
}

fn edit_step(mut m: EditModal, ev: Event, area: Rect) -> ModalStep {
    if let Event::Key(k) = &ev {
        match (k.code, k.modifiers) {
            // Esc layering: an active selection in the focused field cancels
            // first; only once there's no selection left does Esc fall
            // through to the next layer (dropdown, then the modal itself).
            // Gated on `is_selecting()` (not `selection_range()`) — even a
            // degenerate zero-length selection (cursor back at its anchor)
            // is still a selection *process* worth Esc-cancelling, though
            // there's nothing visibly highlighted to show for it.
            (KeyCode::Esc, _) => {
                if let Some(ta) = focused_textarea_mut(&mut m) {
                    if ta.is_selecting() {
                        ta.cancel_selection();
                        return ModalStep::Continue(Modal::Edit(m));
                    }
                }
                if m.focus == EditFocus::Category && m.dropdown.is_some() {
                    m.dropdown = None;
                    return ModalStep::Continue(Modal::Edit(m));
                }
                return ModalStep::Continue(Modal::None);
            }
            (KeyCode::Tab, _) => {
                m.dropdown = None;
                m.cycle_focus();
                m.park_focused_field_at_end();
                return ModalStep::Continue(Modal::Edit(m));
            }
            // Terminals report Shift+Tab as its own BackTab key.
            (KeyCode::BackTab, _) => {
                m.dropdown = None;
                m.cycle_focus_back();
                m.park_focused_field_at_end();
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Char('s'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                return ModalStep::Save(m);
            }
            (KeyCode::Char('d'), mods)
                if mods.contains(KeyModifiers::CONTROL) && m.original.is_some() =>
            {
                return ModalStep::Continue(Modal::ConfirmDelete(m));
            }
            // Ctrl+C/Ctrl+X: native `TextArea::copy`/`cut` do the in-memory
            // work (selection → yank buffer); we additionally stash the
            // yanked text in `pending_clipboard` so `App::handle_modal_event`
            // can push it to the SYSTEM clipboard (`edit_step` stays pure —
            // no process spawning here). Ctrl+V is handled separately below
            // (native binding doesn't do what we want — see `paste_focused`).
            (KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                m.pending_clipboard = copy_focused(&mut m);
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Char('x'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                m.pending_clipboard = cut_focused(&mut m);
                if m.focus == EditFocus::Category {
                    m.dropdown = None; // cut can change the category text
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Char('v'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                paste_focused(&mut m);
                if m.focus == EditFocus::Category {
                    m.dropdown = None; // paste can change the category text
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Down, _) if m.focus == EditFocus::Category => {
                // Clamp against the ROWS ACTUALLY DRAWN, not just the cap: at
                // a degenerate terminal size the dropdown rect is clipped by
                // `.intersection(term_area)`, and the highlight must never
                // point at a row the user cannot see (Enter would accept an
                // invisible suggestion). `visible_rows` already subsumes both
                // the MAX_DROPDOWN_ROWS cap and filtered().len() — dropdown_rows()
                // (which feeds edit_layout here) is itself capped by both.
                let n = visible_rows(&m, area);
                if n > 0 {
                    m.dropdown = Some(m.dropdown.map_or(0, |i| (i + 1).min(n - 1)));
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Up, _) if m.focus == EditFocus::Category => {
                m.dropdown = match m.dropdown {
                    Some(i) if i > 0 => Some(i - 1),
                    _ => None,
                };
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Enter, _) if m.focus == EditFocus::Category => {
                match m.dropdown {
                    // Only accept a highlight that is actually drawn — a
                    // resize between the Down that set it and this Enter can
                    // leave `i` pointing past what's visible now (B1 review
                    // follow-up). Treat that as no-highlight: close the
                    // dropdown without accepting, and stay on Category
                    // rather than silently advancing to Title.
                    Some(i) if i < visible_rows(&m, area) => m.accept_suggestion(i),
                    Some(_) => m.dropdown = None,
                    None => {
                        m.focus = EditFocus::Title;
                        m.sync_blocks();
                        m.park_focused_field_at_end();
                    }
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Enter, _) if m.focus == EditFocus::Title => {
                m.focus = EditFocus::Body;
                m.sync_blocks();
                m.park_focused_field_at_end();
                return ModalStep::Continue(Modal::Edit(m));
            }
            // Cheap keyboard reachability for the button row (mouse click
            // is the primary requirement; Enter activates whichever button
            // Tab last landed on).
            (KeyCode::Enter, _) if m.focus == EditFocus::Save => return ModalStep::Save(m),
            (KeyCode::Enter, _) if m.focus == EditFocus::Cancel => {
                return ModalStep::Continue(Modal::None)
            }
            (KeyCode::Enter, _) if m.focus == EditFocus::Delete => {
                return ModalStep::Continue(Modal::ConfirmDelete(m))
            }
            _ => {}
        }
    }
    if let Event::Mouse(mev) = &ev {
        if mev.kind == MouseEventKind::Down(MouseButton::Left) {
            let is_edit = m.original.is_some();
            let layout = edit_layout(area, is_edit, m.dropdown_rows());
            return match edit_click(&layout, is_edit, mev.column, mev.row) {
                Some(EditClickTarget::Suggestion(row)) => {
                    m.accept_suggestion(row as usize);
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Category) => {
                    m.focus = EditFocus::Category;
                    m.dropdown = None;
                    m.sync_blocks();
                    // A plain click always clears any existing selection
                    // first — otherwise `jump_cursor_to_click`'s
                    // `move_cursor(Jump(..))` would inherit
                    // `shift = selection_start.is_some()` and EXTEND the
                    // stale selection to the click point instead of just
                    // repositioning the cursor there (normal editor
                    // behavior: a non-shift click always replaces the
                    // selection).
                    m.category.cancel_selection();
                    jump_cursor_to_click(&mut m.category, layout.category, mev.column, mev.row);
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Title) => {
                    m.focus = EditFocus::Title;
                    m.dropdown = None;
                    m.sync_blocks();
                    m.title.cancel_selection();
                    jump_cursor_to_click(&mut m.title, layout.title, mev.column, mev.row);
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Body) => {
                    m.focus = EditFocus::Body;
                    m.dropdown = None;
                    m.sync_blocks();
                    m.body.cancel_selection();
                    jump_cursor_to_click(&mut m.body, layout.body, mev.column, mev.row);
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Save) => ModalStep::Save(m),
                Some(EditClickTarget::Cancel) => ModalStep::Continue(Modal::None),
                Some(EditClickTarget::Delete) => ModalStep::Continue(Modal::ConfirmDelete(m)),
                None => ModalStep::Continue(Modal::Edit(m)),
            };
        }
        // Mouse drag selection: only the FOCUSED field's own drag extends a
        // selection (a drag that wanders outside it is ignored, not
        // redirected — dragging into a different field shouldn't steal
        // focus or start a second selection there). Reuses
        // `jump_cursor_to_click`'s math and its horizontal-scroll caveat.
        // Lazily starts the anchor at the CURRENT cursor position (which the
        // preceding Down click already placed) the first time a Drag arrives
        // with no active (non-empty) selection yet — `selection_range()`,
        // not `is_selecting()`, so a same-cell first Drag frame re-anchors
        // rather than being mistaken for an already-extending selection.
        if mev.kind == MouseEventKind::Drag(MouseButton::Left) {
            let is_edit = m.original.is_some();
            let layout = edit_layout(area, is_edit, m.dropdown_rows());
            let field_rect = match m.focus {
                EditFocus::Category => Some(layout.category),
                EditFocus::Title => Some(layout.title),
                EditFocus::Body => Some(layout.body),
                EditFocus::Save | EditFocus::Cancel | EditFocus::Delete => None,
            };
            if let Some(rect) = field_rect {
                let inner = Block::bordered().inner(rect);
                if rect_contains(inner, mev.column, mev.row) {
                    if let Some(ta) = focused_textarea_mut(&mut m) {
                        if ta.selection_range().is_none() {
                            ta.start_selection();
                        }
                        jump_cursor_to_click(ta, rect, mev.column, mev.row);
                    }
                }
            }
            return ModalStep::Continue(Modal::Edit(m));
        }
    }
    // Word-jump alias: macOS binds plain Ctrl+←/→ to Mission Control at the
    // OS level, so those keys never reach the terminal there — while
    // Option(Alt)+←/→ is the platform's word-jump muscle memory anyway.
    // tui-textarea 0.7 maps word movement only on Ctrl+arrow (its Alt+arrow
    // slots are unbound; Ctrl+Alt+arrow means line Head/End), so rewrite
    // Alt+←/→ into Ctrl+←/→ before handing the event over. Shift is kept,
    // so Shift+Alt+arrow extends the selection word-wise like Shift+Ctrl.
    let ev = match ev {
        Event::Key(mut k)
            if matches!(k.code, KeyCode::Left | KeyCode::Right)
                && k.modifiers.contains(KeyModifiers::ALT)
                && !k.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            k.modifiers.remove(KeyModifiers::ALT);
            k.modifiers.insert(KeyModifiers::CONTROL);
            Event::Key(k)
        }
        ev => ev,
    };
    match m.focus {
        EditFocus::Category => {
            m.category.input(ev);
            // Typing refilters the list, so a stale highlight must not
            // survive.
            m.dropdown = None;
        }
        EditFocus::Title => {
            m.title.input(ev);
        }
        EditFocus::Body => {
            m.body.input(ev);
        }
        EditFocus::Save | EditFocus::Cancel | EditFocus::Delete => {}
    }
    ModalStep::Continue(Modal::Edit(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::State;
    use crossterm::event::{KeyEvent, KeyModifiers, MouseEvent};

    const TEST_AREA: Rect = Rect::new(0, 0, 80, 24);

    fn down() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
    }

    fn click(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn shift_key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT))
    }

    fn ctrl_key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn drag(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn step_edit(m: EditModal, ev: Event) -> EditModal {
        match step(Modal::Edit(m), ev, TEST_AREA) {
            ModalStep::Continue(Modal::Edit(m)) => m,
            _ => panic!("expected Continue(Edit), got another step"),
        }
    }

    fn create_modal() -> EditModal {
        EditModal::create(vec![])
    }

    fn edit_modal() -> EditModal {
        EditModal::edit(
            ItemKey {
                title: "A".into(),
                state: State::Open,
            },
            &Item {
                state: State::Open,
                title: "A".into(),
                agent: None,
                done_date: None,
                body: vec![],
            },
            None,
            vec![],
        )
    }

    fn as_edit(step_result: ModalStep) -> EditModal {
        match step_result {
            ModalStep::Continue(Modal::Edit(m)) => m,
            _ => panic!("expected Continue(Modal::Edit(_))"),
        }
    }

    /// Rider from Task 10's review: the viewer scroll `Down` arms used plain
    /// `scroll + 1` (u16), which debug-panics on overflow at 65,535 presses.
    /// Hammer both DoneView and FileView well past u16::MAX to prove
    /// `saturating_add` holds instead of panicking.
    #[test]
    fn done_view_scroll_down_saturates_past_u16_max() {
        let mut modal = Modal::DoneView {
            scroll: u16::MAX - 5,
        };
        for _ in 0..70_000 {
            modal = match step(modal, down(), TEST_AREA) {
                ModalStep::Continue(m) => m,
                _ => panic!("DoneView Down must stay in Continue"),
            };
        }
        assert!(matches!(modal, Modal::DoneView { scroll } if scroll == u16::MAX));
    }

    #[test]
    fn file_view_scroll_down_saturates_past_u16_max() {
        let mut modal = Modal::FileView {
            scroll: u16::MAX - 5,
        };
        for _ in 0..70_000 {
            modal = match step(modal, down(), TEST_AREA) {
                ModalStep::Continue(m) => m,
                _ => panic!("FileView Down must stay in Continue"),
            };
        }
        assert!(matches!(modal, Modal::FileView { scroll } if scroll == u16::MAX));
    }

    #[test]
    fn edit_layout_has_no_delete_button_in_create_mode_and_one_in_edit_mode() {
        assert!(edit_layout(TEST_AREA, false, 0).delete.is_none());
        assert!(edit_layout(TEST_AREA, true, 0).delete.is_some());
    }

    /// Crash regression: at narrow widths (e.g. the ~30-col pane herdr docks
    /// a sidebar into) the button row's raw `x`-cursor arithmetic — Save
    /// width + gap + Cancel width + gap + Delete width, added up with no
    /// bound check — placed the Delete (and sometimes Cancel) button's Rect
    /// entirely past the right edge of the terminal itself, not just the
    /// modal panel. `view::draw_edit_modal` then handed that Rect straight
    /// to `Paragraph::render`, which indexes the frame's buffer directly and
    /// panics ("index outside of buffer") the instant `x >= buffer width` —
    /// killing the whole pane. Every button rect must stay within the
    /// terminal area at every width, not just the ones wide enough for all
    /// three labels to fit comfortably. A rect clipped down to zero width is
    /// exempt — `Rect::right()` on an empty rect can still report a large
    /// `x`, but an empty rect is never actually drawn into or clickable
    /// (`Paragraph::render_paragraph` and `rect_contains` both treat
    /// zero-width as a no-op), so it can't cause the out-of-buffer write.
    ///
    /// B2 review follow-up: extended past the original width-only sweep to
    /// also vary `dropdown_rows` (0 and the max 5) and a couple of short
    /// heights (10, 12 — the B1 short-pane collapse range), asserting the
    /// `dropdown` rect itself stays within the terminal's right/bottom edges
    /// too, the same way the buttons already were.
    #[test]
    fn edit_layout_rects_never_escape_the_terminal() {
        for width in 1..=120u16 {
            for height in [10u16, 12, 40] {
                let area = Rect::new(0, 0, width, height);
                for is_edit in [false, true] {
                    for dropdown_rows in [0u16, 5] {
                        let layout = edit_layout(area, is_edit, dropdown_rows);
                        for (name, r) in [("save", layout.save), ("cancel", layout.cancel)] {
                            assert!(
                                r.is_empty() || r.right() <= area.width,
                                "{name} button {r:?} escapes terminal width {width}"
                            );
                        }
                        if let Some(d) = layout.delete {
                            assert!(
                                d.is_empty() || d.right() <= area.width,
                                "delete button {d:?} escapes terminal width {width}"
                            );
                        }
                        let d = layout.dropdown;
                        assert!(
                            d.is_empty() || (d.right() <= area.width && d.bottom() <= area.height),
                            "dropdown {d:?} escapes terminal {width}x{height}"
                        );
                        let di = layout.dropdown_inner;
                        assert!(
                            di.is_empty()
                                || (di.right() <= area.width && di.bottom() <= area.height),
                            "dropdown inner {di:?} escapes terminal {width}x{height}"
                        );
                    }
                }
            }
        }
    }

    /// Category is the topmost field (spec 2026-09-16 §2), focused first on
    /// create; clicking Title or Body moves focus there directly.
    #[test]
    fn clicking_title_or_body_moves_focus_there() {
        let layout = edit_layout(TEST_AREA, false, 0);
        let mut m = create_modal();
        assert_eq!(m.focus, EditFocus::Category);
        m.cycle_focus(); // -> Title
        assert_eq!(m.focus, EditFocus::Title);

        let m = as_edit(step(
            Modal::Edit(m),
            click(layout.body.x, layout.body.y),
            TEST_AREA,
        ));
        assert_eq!(m.focus, EditFocus::Body);

        let m = as_edit(step(
            Modal::Edit(m),
            click(layout.title.x, layout.title.y),
            TEST_AREA,
        ));
        assert_eq!(m.focus, EditFocus::Title);
    }

    #[test]
    fn clicking_save_button_saves() {
        let layout = edit_layout(TEST_AREA, false, 0);
        let m = create_modal();
        let result = step(
            Modal::Edit(m),
            click(layout.save.x, layout.save.y),
            TEST_AREA,
        );
        assert!(matches!(result, ModalStep::Save(_)));
    }

    #[test]
    fn clicking_cancel_button_closes_modal() {
        let layout = edit_layout(TEST_AREA, false, 0);
        let m = create_modal();
        let result = step(
            Modal::Edit(m),
            click(layout.cancel.x, layout.cancel.y),
            TEST_AREA,
        );
        assert!(matches!(result, ModalStep::Continue(Modal::None)));
    }

    #[test]
    fn clicking_delete_button_opens_confirm_delete() {
        let layout = edit_layout(TEST_AREA, true, 0);
        let m = edit_modal();
        let delete = layout.delete.expect("edit mode has a delete button");
        let result = step(Modal::Edit(m), click(delete.x, delete.y), TEST_AREA);
        assert!(matches!(
            result,
            ModalStep::Continue(Modal::ConfirmDelete(_))
        ));
    }

    #[test]
    fn tab_reaches_save_cancel_and_delete_and_enter_activates_them() {
        // Create mode: Category -> Title -> Body -> Save -> Cancel -> Category.
        let mut m = create_modal();
        for _ in 0..3 {
            m.cycle_focus();
        }
        assert_eq!(m.focus, EditFocus::Save);
        assert!(matches!(
            step(Modal::Edit(m), enter(), TEST_AREA),
            ModalStep::Save(_)
        ));

        let mut m = create_modal();
        for _ in 0..4 {
            m.cycle_focus();
        }
        assert_eq!(m.focus, EditFocus::Cancel);
        assert!(matches!(
            step(Modal::Edit(m), enter(), TEST_AREA),
            ModalStep::Continue(Modal::None)
        ));

        // Edit mode: Category -> Title -> Body -> Save -> Cancel -> Delete.
        let mut m = edit_modal();
        for _ in 0..5 {
            m.cycle_focus();
        }
        assert_eq!(m.focus, EditFocus::Delete);
        assert!(matches!(
            step(Modal::Edit(m), enter(), TEST_AREA),
            ModalStep::Continue(Modal::ConfirmDelete(_))
        ));
    }

    /// Cursor fix: tui-textarea renders its own cursor cell using whatever
    /// `cursor_style` is set — always at the viewport-correct position,
    /// unlike a real terminal cursor placed from the logical column. Only
    /// the FOCUSED field should get a visible (reversed) cursor style; the
    /// other two must stay invisible, on construction and after every focus
    /// change.
    #[test]
    fn focused_field_gets_reversed_cursor_style_others_stay_invisible() {
        let visible = Style::default().add_modifier(Modifier::REVERSED);
        let invisible = Style::default();

        let m = create_modal(); // focus starts on Category
        assert_eq!(m.category.cursor_style(), visible);
        assert_eq!(m.title.cursor_style(), invisible);
        assert_eq!(m.body.cursor_style(), invisible);

        let mut m = edit_modal(); // focus also starts on Category
        assert_eq!(m.category.cursor_style(), visible);
        assert_eq!(m.title.cursor_style(), invisible);
        assert_eq!(m.body.cursor_style(), invisible);
        m.cycle_focus(); // Category -> Title
        assert_eq!(m.category.cursor_style(), invisible);
        assert_eq!(m.title.cursor_style(), visible);
        assert_eq!(m.body.cursor_style(), invisible);
    }

    #[test]
    fn create_focuses_category_first_and_cycles_through_it() {
        let mut m = EditModal::create(vec!["Work".into()]);
        assert_eq!(m.focus, EditFocus::Category);
        m.cycle_focus();
        assert_eq!(m.focus, EditFocus::Title);
        m.cycle_focus(); // Body
        m.cycle_focus(); // Save
        m.cycle_focus(); // Cancel
        m.cycle_focus(); // back to Category (create mode: no Delete)
        assert_eq!(m.focus, EditFocus::Category);
    }

    #[test]
    fn filtered_matches_substring_case_insensitive() {
        let mut m = EditModal::create(vec!["Work".into(), "Chores".into(), "Homework".into()]);
        assert_eq!(m.filtered(), vec!["Work", "Chores", "Homework"]); // empty query = all
        m.category.insert_str("ork");
        assert_eq!(m.filtered(), vec!["Work", "Homework"]);
        m.set_category("Chores");
        assert_eq!(m.category_text(), "Chores");
    }

    /// Review follow-up (B1): at very short pane heights the constraint
    /// solver starves Title if Category keeps its full 3-row height —
    /// collapsing Category to 0 when the inner area is too short keeps Title
    /// usable instead.
    ///
    /// Height 14 (not the plan's illustrative 12) is the boundary the test
    /// actually needs: at inner height 8 (this rect's inner area), Category
    /// collapsing to 0 frees exactly enough room for Title(3) + Body's own
    /// Min(3) + the button/hint rows(1+1) to all fit. At inner heights below
    /// 8 (e.g. height 12, giving inner 6) there simply isn't enough room for
    /// Title to reach 3 rows *and* Body to keep its Min(3) floor no matter
    /// what Category does — ratatui's solver shrinks Title's `Length(3)`
    /// before it violates Body's `Min(3)`, so Title can only ever be made
    /// "usable" down to this floor, not below it.
    #[test]
    fn short_pane_collapses_category_keeps_title_usable() {
        let l = edit_layout(Rect::new(0, 0, 30, 14), false, 0);
        assert_eq!(l.category.height, 0, "category collapses on short panes");
        assert!(
            l.title.height >= 3,
            "title must stay usable, got {}",
            l.title.height
        );
    }

    /// Tall enough panes keep Category at its full height — the collapse is
    /// a short-pane-only concession.
    #[test]
    fn tall_pane_keeps_category_at_full_height() {
        let l = edit_layout(TEST_AREA, false, 0);
        assert_eq!(l.category.height, 3);
    }

    #[test]
    fn layout_stacks_category_above_title_and_sizes_dropdown() {
        let l = edit_layout(TEST_AREA, false, 3);
        assert!(l.category.y < l.title.y && l.title.y < l.body.y);
        // Bordered box: 3 suggestion rows + top/bottom border = 5.
        assert_eq!(l.dropdown.height, 5);
        assert_eq!(l.dropdown_inner.height, 3);
        assert_eq!(l.dropdown.y, l.category.y + l.category.height);
        assert_eq!(
            l.dropdown_inner.y,
            l.dropdown.y + 1,
            "inner area starts past the top border"
        );
        let l0 = edit_layout(TEST_AREA, false, 0);
        assert_eq!(l0.dropdown.height, 0, "closed dropdown draws no border");
        assert_eq!(l0.dropdown_inner.height, 0);
    }

    #[test]
    fn clicks_hit_category_and_dropdown_rows() {
        let l = edit_layout(TEST_AREA, false, 2);
        assert_eq!(
            edit_click(&l, false, l.category.x + 1, l.category.y + 1),
            Some(EditClickTarget::Category)
        );
        // The dropdown overlays the title area — it must win the hit-test.
        // Row 1 is the second drawn suggestion row, inside the bordered
        // box's inner (row-bearing) area.
        assert_eq!(
            edit_click(&l, false, l.dropdown_inner.x, l.dropdown_inner.y + 1),
            Some(EditClickTarget::Suggestion(1))
        );
        // A click on the dropdown's own border is consumed as a no-op — it
        // must neither pick a suggestion nor fall through to Category/Title
        // beneath it.
        assert_eq!(
            edit_click(&l, false, l.dropdown.x, l.dropdown.y),
            None,
            "a border click on the dropdown must be a no-op"
        );
        let l0 = edit_layout(TEST_AREA, false, 0);
        assert_eq!(
            edit_click(&l0, false, l0.title.x + 1, l0.title.y + 1),
            Some(EditClickTarget::Title)
        );
    }

    #[test]
    fn edit_prefills_category_from_section() {
        let item = Item {
            state: State::Open,
            title: "T".into(),
            agent: None,
            done_date: None,
            body: Vec::new(),
        };
        let key = ItemKey {
            title: "T".into(),
            state: State::Open,
        };
        let m = EditModal::edit(key.clone(), &item, Some("Work".into()), vec!["Work".into()]);
        assert_eq!(m.category_text(), "Work");
        assert_eq!(m.original_category.as_deref(), Some("Work"));
        // Already categorized → Title focused (no unprompted dropdown).
        assert_eq!(m.focus, EditFocus::Title);
        let m = EditModal::edit(key, &item, None, vec![]);
        assert_eq!(m.category_text(), "");
        assert_eq!(m.original_category, None);
        // Uncategorized → Category focused, like create.
        assert_eq!(m.focus, EditFocus::Category);
    }

    /// tui-textarea underlines the cursor's whole line by default, which in
    /// these mostly-single-line fields read as "all text is underlined".
    #[test]
    fn no_field_underlines_its_cursor_line() {
        let m = EditModal::create(vec!["Work".into()]);
        for ta in [&m.category, &m.title, &m.body] {
            assert_eq!(ta.cursor_line_style(), Style::default());
        }
    }

    /// Fix: opening an existing item must leave every field's viewport
    /// scrolled to the START, not the tail. `EditModal::edit` used to park
    /// all three cursors at End/Bottom so "appending is the common edit"
    /// stayed a one-keystroke-away default — but the modal's FIRST frame
    /// renders at the narrow pre-zoom pane width, and tui-textarea scrolls
    /// its viewport to keep a parked cursor visible on that first frame; it
    /// never scrolls back once the pane later widens. Net effect: opening a
    /// task with any long line showed it clipped to its tail, and a
    /// multi-line body scrolled to its bottom, even though nothing was
    /// focused there yet. Parking at start keeps the viewport pinned to
    /// (0,0) from the first frame — the end is still one Ctrl+E / End-key /
    /// click away.
    #[test]
    fn edit_parks_all_cursors_at_start_not_end() {
        let item = Item {
            state: State::Open,
            title: "A very long title that would have scrolled the field horizontally".into(),
            agent: None,
            done_date: None,
            body: vec![
                "A very long first line of body text that would have scrolled".into(),
                "second line".into(),
                "third line".into(),
            ],
        };
        let key = ItemKey {
            title: "A".into(),
            state: State::Open,
        };
        let m = EditModal::edit(key, &item, Some("Work".into()), vec!["Work".into()]);
        assert_eq!(m.category.cursor(), (0, 0), "category must park at start");
        assert_eq!(m.title.cursor(), (0, 0), "title must park at start");
        assert_eq!(m.body.cursor(), (0, 0), "body must park at start");
    }

    /// Spec review follow-up: at a degenerate terminal size the dropdown
    /// rect is clipped by `.intersection(term_area)` in `edit_layout` to
    /// fewer rows than `filtered().len().min(MAX_DROPDOWN_ROWS)`. Keyboard
    /// Down must clamp against what's actually drawn, not just the cap —
    /// otherwise the highlight can point at a row that never renders, and
    /// Enter silently accepts a suggestion the user never saw.
    #[test]
    fn down_never_highlights_a_clipped_off_row() {
        // 15x5 terminal: the dropdown rect is clipped to fewer rows than the
        // filtered list — the highlight must stay within what is drawn.
        let tiny = Rect::new(0, 0, 15, 5);
        let names: Vec<String> = ["A", "B", "C", "D", "E", "F"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut m = EditModal::create(names);
        let visible = edit_layout(tiny, false, m.dropdown_rows())
            .dropdown_inner
            .height as usize;
        assert!(
            visible < m.filtered().len().min(MAX_DROPDOWN_ROWS),
            "fixture must actually clip"
        );
        for _ in 0..5 {
            m = match step(Modal::Edit(m), key(KeyCode::Down), tiny) {
                ModalStep::Continue(Modal::Edit(m)) => m,
                _ => panic!("expected Continue(Edit)"),
            };
        }
        match m.dropdown {
            Some(i) => assert!(
                i < visible,
                "highlight {i} points past {visible} drawn rows"
            ),
            None => assert_eq!(visible, 0, "None only acceptable when nothing is drawn"),
        }
    }

    /// B1 review follow-up: the Enter arm used to accept `m.dropdown = Some(i)`
    /// without re-checking visibility, so a resize between the Down that set
    /// the highlight and the Enter that accepts it could accept a
    /// clipped-off (unseen) suggestion. Highlight a deep row on a tall
    /// terminal, then feed the accepting Enter with a tiny area — the
    /// category must stay untouched and the dropdown must simply close.
    #[test]
    fn resized_enter_never_accepts_a_now_invisible_highlight() {
        let tall = Rect::new(0, 0, 80, 40);
        let names: Vec<String> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut m = EditModal::create(names);
        for _ in 0..5 {
            m = match step(Modal::Edit(m), key(KeyCode::Down), tall) {
                ModalStep::Continue(Modal::Edit(m)) => m,
                _ => panic!("expected Continue(Edit)"),
            };
        }
        assert_eq!(m.dropdown, Some(4), "highlight parked on the deepest row");

        // Terminal shrinks between the Down and the Enter — now the same
        // dropdown rect draws fewer rows than the highlighted index.
        let tiny = Rect::new(0, 0, 15, 5);
        assert!(
            visible_rows(&m, tiny) <= 4,
            "fixture must actually clip below the highlighted row"
        );

        match step(Modal::Edit(m), enter(), tiny) {
            ModalStep::Continue(Modal::Edit(m)) => {
                assert_eq!(
                    m.category_text(),
                    "",
                    "must not accept an unseen suggestion"
                );
                assert_eq!(
                    m.dropdown, None,
                    "dropdown closes rather than silently accepting"
                );
            }
            _ => panic!("expected Continue(Edit)"),
        }
    }

    #[test]
    fn down_enters_dropdown_enter_accepts() {
        let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
        let m = step_edit(m, key(KeyCode::Down));
        assert_eq!(m.dropdown, Some(0));
        let m = step_edit(m, key(KeyCode::Down));
        assert_eq!(m.dropdown, Some(1)); // clamped at the end, not wrapping
        let m = step_edit(m, key(KeyCode::Down));
        assert_eq!(m.dropdown, Some(1));
        let m = step_edit(m, key(KeyCode::Enter));
        assert_eq!(m.category_text(), "Chores");
        assert_eq!(m.dropdown, None);
        assert_eq!(m.focus, EditFocus::Category, "accept stays on the field");
    }

    #[test]
    fn enter_with_dropdown_closed_advances_to_title() {
        let m = EditModal::create(vec!["Work".into()]);
        let m = step_edit(m, key(KeyCode::Enter));
        assert_eq!(m.focus, EditFocus::Title);
        // Enter-advance parks like Tab does (end of an empty field = (0,0)).
        assert_eq!(m.title.cursor(), (0, 0));
    }

    #[test]
    fn esc_closes_dropdown_first_then_cancels() {
        let m = EditModal::create(vec!["Work".into()]);
        let m = step_edit(m, key(KeyCode::Down));
        assert_eq!(m.dropdown, Some(0));
        let m = step_edit(m, key(KeyCode::Esc)); // first Esc: dropdown only
        assert_eq!(m.dropdown, None);
        match step(Modal::Edit(m), key(KeyCode::Esc), TEST_AREA) {
            ModalStep::Continue(Modal::None) => {} // second Esc: modal cancels
            _ => panic!("second Esc must cancel the modal"),
        }
    }

    #[test]
    fn typing_filters_and_resets_highlight_up_leaves_list() {
        let m = EditModal::create(vec!["Work".into(), "Homework".into(), "Chores".into()]);
        let m = step_edit(m, key(KeyCode::Down));
        let m = step_edit(m, key(KeyCode::Char('w')));
        assert_eq!(m.dropdown, None, "typing resets the highlight");
        assert_eq!(m.filtered(), vec!["Work", "Homework"]);
        let m = step_edit(m, key(KeyCode::Down));
        let m = step_edit(m, key(KeyCode::Up));
        assert_eq!(m.dropdown, None, "Up at the top leaves the list");
    }

    #[test]
    fn tab_closes_dropdown_and_moves_focus() {
        let m = EditModal::create(vec!["Work".into()]);
        let m = step_edit(m, key(KeyCode::Down));
        let m = step_edit(m, key(KeyCode::Tab));
        assert_eq!(m.dropdown, None);
        assert_eq!(m.focus, EditFocus::Title);
    }

    fn categorized_edit_modal() -> EditModal {
        let item = Item {
            state: State::Open,
            title: "abc".into(),
            agent: None,
            done_date: None,
            body: vec!["alpha beta gamma".into(), "line two".into()],
        };
        let key = ItemKey {
            title: "abc".into(),
            state: State::Open,
        };
        EditModal::edit(key, &item, Some("Work".into()), vec!["Work".into()])
    }

    /// Standard form behavior: tabbing into a field parks its cursor at the
    /// end of the field's content (construction still parks at the start —
    /// that's the open-viewport fix; the two are deliberately different).
    #[test]
    fn tab_parks_cursor_at_end_of_entered_field() {
        let m = categorized_edit_modal(); // focus starts on Title
        assert_eq!(m.title.cursor(), (0, 0), "construction parks at start");
        let m = step_edit(m, key(KeyCode::Tab)); // → Body
        assert_eq!(m.focus, EditFocus::Body);
        assert_eq!(m.body.cursor(), (1, "line two".len()), "end of content");
    }

    #[test]
    fn back_tab_cycles_backwards_and_parks_at_end() {
        let m = categorized_edit_modal(); // focus starts on Title
        let m = step_edit(m, key(KeyCode::BackTab)); // ← Category
        assert_eq!(m.focus, EditFocus::Category);
        assert_eq!(m.category.cursor(), (0, "Work".len()));
        let m = step_edit(m, key(KeyCode::BackTab)); // ← Delete (edit mode)
        assert_eq!(m.focus, EditFocus::Delete);
        // Create mode wraps Category → Cancel (no Delete):
        let c = EditModal::create(vec![]);
        let c = step_edit(c, key(KeyCode::BackTab));
        assert_eq!(c.focus, EditFocus::Cancel);
    }

    /// Tabbing into a field with a stale selection must not extend it to the
    /// end — the park drops the selection first.
    #[test]
    fn tab_into_field_drops_stale_selection() {
        let m = categorized_edit_modal(); // focus Title, "abc"
        let m = step_edit(
            m,
            Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
        );
        assert!(m.title.selection_range().is_some());
        let m = step_edit(m, key(KeyCode::BackTab)); // away (→ Category)
        let m = step_edit(m, key(KeyCode::Tab)); // back into Title
        assert_eq!(m.focus, EditFocus::Title);
        assert_eq!(m.title.selection_range(), None);
        assert_eq!(m.title.cursor(), (0, "abc".len()));
    }

    /// macOS Mission Control eats plain Ctrl+←/→ before the terminal sees
    /// them, so Alt(Option)+←/→ — the platform's own word-jump keys — are
    /// remapped onto tui-textarea's Ctrl+arrow word movement. Shift is
    /// preserved, so Shift+Alt+→ selects word-wise.
    #[test]
    fn alt_arrows_jump_by_word_shift_extends_selection() {
        let mut m = categorized_edit_modal();
        m.focus = EditFocus::Body; // "alpha beta gamma", cursor (0,0)
        m.sync_blocks();
        let m = step_edit(
            m,
            Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
        );
        assert_eq!(m.body.cursor(), (0, 6), "start of \"beta\"");
        let m = step_edit(
            m,
            Event::Key(KeyEvent::new(
                KeyCode::Right,
                KeyModifiers::ALT | KeyModifiers::SHIFT,
            )),
        );
        assert_eq!(m.body.cursor(), (0, 11), "start of \"gamma\"");
        assert!(
            m.body.selection_range().is_some(),
            "shifted word-jump must select"
        );
    }

    /// B3 review follow-up: a click that moves focus off Category (to Title
    /// or Body) must also clear a stale dropdown highlight — Tab already did
    /// this, but a focus-moving click didn't.
    #[test]
    fn clicking_title_or_body_clears_stale_dropdown_highlight() {
        let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
        let m = step_edit(m, key(KeyCode::Down)); // dropdown open
        assert_eq!(m.dropdown, Some(0));
        let layout = edit_layout(TEST_AREA, false, m.dropdown_rows());
        // The dropdown's bordered box shares Category/Title's width and, for
        // 2 suggestions, is taller than Title's own 3 rows — it overlays
        // all of Title and spills one row into Body. Click just past where
        // the dropdown box ends so it actually lands on Body (not consumed
        // by the dropdown's own border, and not Title, which is entirely
        // covered here).
        let y = layout.dropdown.y + layout.dropdown.height;
        let m = step_edit(m, click(layout.body.x + 1, y));
        assert_eq!(m.focus, EditFocus::Body);
        assert_eq!(m.dropdown, None);
    }

    #[test]
    fn click_on_suggestion_accepts_it() {
        let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
        let m = step_edit(m, key(KeyCode::Down)); // dropdown open (2 rows)
        let l = edit_layout(TEST_AREA, false, m.dropdown_rows());
        let m = step_edit(m, click(l.dropdown_inner.x, l.dropdown_inner.y + 1));
        assert_eq!(m.category_text(), "Chores");
        assert_eq!(m.dropdown, None);
    }

    /// Task 2: word-by-word navigation. `edit_step` doesn't intercept
    /// Left/Right with any modifier, so a Ctrl+Right/Ctrl+Left reaches
    /// tui-textarea's `input()` unchanged, which maps it to
    /// `CursorMove::WordForward`/`WordBack` (verified against the vendored
    /// source: `~/.cargo/registry/src/index.crates.io-*/tui-textarea-0.7.0/
    /// src/textarea.rs`, the `Key::Right, ctrl: true, alt: false` arm of
    /// `input()`, mirrored by `Key::Left` for `WordBack`; also reachable via
    /// Alt+f/Alt+b). `WordForward` (`cursor.rs`, via
    /// `word::find_word_start_forward`) lands on the START of the NEXT word,
    /// not the end of the current one — observed and pinned here: from Head
    /// on "alpha beta gamma", Ctrl+Right lands at column 6, the `b` of
    /// "beta" ("alpha " is 6 columns: a-l-p-h-a-space).
    #[test]
    fn ctrl_right_jumps_forward_a_word_in_body() {
        let mut m = edit_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["alpha beta gamma".to_string()]);
        m.body.move_cursor(CursorMove::Head);
        m.sync_blocks();
        let ev = Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        let m = step_edit(m, ev);
        assert_eq!(
            m.body.cursor(),
            (0, 6),
            "Ctrl+Right must land at the start of \"beta\""
        );
    }

    /// Same check once for Category (single-line) to prove none of the
    /// Category-only key arms (Down/Up for the dropdown, Enter) shadow
    /// Ctrl+Right — it must still reach tui-textarea's word-forward
    /// unshadowed.
    #[test]
    fn ctrl_right_jumps_forward_a_word_in_category() {
        let mut m = create_modal(); // focus starts on Category
        m.category = TextArea::new(vec!["alpha beta gamma".to_string()]);
        m.category.move_cursor(CursorMove::Head);
        m.sync_blocks();
        let ev = Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        let m = step_edit(m, ev);
        assert_eq!(
            m.category.cursor(),
            (0, 6),
            "Ctrl+Right must reach tui-textarea's word-forward unshadowed by Category's own key arms"
        );
    }

    // --- Task 3: click-to-position cursor -----------------------------------

    #[test]
    fn clicking_body_text_places_cursor_at_the_clicked_character() {
        let mut m = create_modal();
        m.body = TextArea::new(vec!["hello world".to_string()]);
        let layout = edit_layout(TEST_AREA, false, 0);
        let inner = Block::bordered().inner(layout.body);
        // Column 6 is the 'w' of "world" ("hello " is 6 columns wide).
        let m = step_edit(m, click(inner.x + 6, inner.y));
        assert_eq!(m.focus, EditFocus::Body);
        assert_eq!(m.body.cursor(), (0, 6));
    }

    #[test]
    fn clicking_past_end_of_body_text_clamps_cursor_to_line_end() {
        let mut m = create_modal();
        m.body = TextArea::new(vec!["hello world".to_string()]);
        let layout = edit_layout(TEST_AREA, false, 0);
        let inner = Block::bordered().inner(layout.body);
        // Last column of the (much wider than the text) inner rect — well
        // past "hello world"'s 11 characters, but still inside the field so
        // the click actually lands on Body rather than missing it.
        let m = step_edit(m, click(inner.x + inner.width - 1, inner.y));
        assert_eq!(m.focus, EditFocus::Body);
        assert_eq!(
            m.body.cursor(),
            (0, "hello world".chars().count()),
            "CursorMove::Jump clamps a past-end column to the line's end"
        );
    }

    /// Clicking the field's own BORDER (not its inner text area) must still
    /// move focus there like today, but must NOT jump the cursor — there is
    /// no clicked character to jump to.
    #[test]
    fn clicking_body_border_moves_focus_but_leaves_cursor_unchanged() {
        let mut m = create_modal();
        m.body = TextArea::new(vec!["hello world".to_string()]);
        m.body.move_cursor(CursorMove::Jump(0, 5));
        let layout = edit_layout(TEST_AREA, false, 0);
        // layout.body's own (x, y) is the top-left corner of its border.
        let m = step_edit(m, click(layout.body.x, layout.body.y));
        assert_eq!(m.focus, EditFocus::Body);
        assert_eq!(
            m.body.cursor(),
            (0, 5),
            "a border click must not move the cursor"
        );
    }

    #[test]
    fn clicking_category_text_places_cursor_and_resets_dropdown() {
        let mut m = create_modal();
        m.category = TextArea::new(vec!["alpha beta".to_string()]);
        m.dropdown = Some(0); // simulate an open dropdown highlight
        let layout = edit_layout(TEST_AREA, false, 0);
        let inner = Block::bordered().inner(layout.category);
        // Column 6 is the 'b' of "beta" ("alpha " is 6 columns wide).
        let m = step_edit(m, click(inner.x + 6, inner.y));
        assert_eq!(m.focus, EditFocus::Category);
        assert_eq!(m.category.cursor(), (0, 6));
        assert_eq!(
            m.dropdown, None,
            "existing behavior preserved: a field click resets the dropdown highlight"
        );
    }

    // --- Text selection: keyboard, mouse drag, clipboard -------------------

    /// tui-textarea 0.7's native `input()` already maps Shift+Right to
    /// `move_cursor_with_shift(CursorMove::Forward, true)`, which lazily
    /// calls `start_selection()` the first time shift is held — nothing in
    /// `edit_step` intercepts Right with any modifier, so this reaches
    /// tui-textarea unshadowed and needs no new code, only this pin.
    #[test]
    fn shift_right_three_times_selects_three_chars_in_body() {
        let mut m = create_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["hello".to_string()]);
        m.sync_blocks();
        for _ in 0..3 {
            m = step_edit(m, shift_key(KeyCode::Right));
        }
        assert_eq!(m.body.selection_range(), Some(((0, 0), (0, 3))));
        m.body.cut();
        assert_eq!(m.body.yank_text(), "hel");
    }

    #[test]
    fn ctrl_c_sets_pending_clipboard_only_when_something_is_selected() {
        let mut m = create_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["hello world".to_string()]);
        m.sync_blocks();

        // No selection: Ctrl+C must leave pending_clipboard untouched.
        let m = step_edit(m, ctrl_key('c'));
        assert_eq!(m.pending_clipboard, None);

        // Select "hello" (Shift+Right x5 from Head) then Ctrl+C.
        let mut m = m;
        for _ in 0..5 {
            m = step_edit(m, shift_key(KeyCode::Right));
        }
        assert!(m.body.selection_range().is_some());
        let m = step_edit(m, ctrl_key('c'));
        assert_eq!(m.pending_clipboard.as_deref(), Some("hello"));
        assert_eq!(m.body.lines(), ["hello world"], "copy never mutates text");
    }

    #[test]
    fn ctrl_x_cuts_selection_and_sets_pending_clipboard() {
        let mut m = create_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["hello world".to_string()]);
        m.sync_blocks();
        for _ in 0..5 {
            m = step_edit(m, shift_key(KeyCode::Right));
        }
        let m = step_edit(m, ctrl_key('x'));
        assert_eq!(m.pending_clipboard.as_deref(), Some("hello"));
        assert_eq!(m.body.lines(), [" world"]);
        assert_eq!(
            m.body.selection_range(),
            None,
            "cut clears the selection it consumed"
        );
    }

    /// Ctrl+V is explicitly intercepted (`paste_focused`) rather than left
    /// to `TextArea::input()`, because tui-textarea 0.7's native binding for
    /// Ctrl+V is `Scrolling::PageDown` (`textarea.rs`'s `Key::Char('v'),
    /// ctrl: true` arm) — its paste key is emacs-style Ctrl+Y instead. This
    /// test pins that native (surprising) mapping as the reason the
    /// interception exists: fed straight through `.input()`, Ctrl+V must NOT
    /// paste — proving `edit_step` really does need its own arm for it.
    #[test]
    fn native_ctrl_v_binding_is_page_down_scroll_not_paste() {
        let mut ta = TextArea::new(vec!["world".to_string()]);
        ta.set_yank_text("hello ");
        ta.move_cursor(CursorMove::Head);
        ta.input(ctrl_key('v'));
        assert_eq!(
            ta.lines(),
            ["world"],
            "native Ctrl+V must not paste — edit_step must intercept it itself"
        );
    }

    #[test]
    fn ctrl_v_pastes_the_internal_yank_buffer() {
        let mut m = create_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["world".to_string()]);
        m.body.set_yank_text("hello ");
        m.body.move_cursor(CursorMove::Head);
        m.sync_blocks();
        let m = step_edit(m, ctrl_key('v'));
        assert_eq!(m.body.lines(), ["hello world"]);
    }

    #[test]
    fn dragging_in_body_selects_the_dragged_span() {
        let mut m = create_modal();
        m.body = TextArea::new(vec!["hello world".to_string()]);
        let layout = edit_layout(TEST_AREA, false, 0);
        let inner = Block::bordered().inner(layout.body);
        // Down at col 0 focuses Body and anchors the (not-yet-started)
        // cursor at (0, 0).
        let m = step_edit(m, click(inner.x, inner.y));
        assert_eq!(m.focus, EditFocus::Body);
        assert_eq!(
            m.body.selection_range(),
            None,
            "a plain click selects nothing yet"
        );
        // Drag to col 5 ('w' of "world") lazily starts the selection at the
        // anchor and extends it to the drag point.
        let m = step_edit(m, drag(inner.x + 5, inner.y));
        assert_eq!(m.body.selection_range(), Some(((0, 0), (0, 5))));
    }

    #[test]
    fn down_click_clears_an_existing_selection() {
        let mut m = create_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["hello world".to_string()]);
        m.sync_blocks();
        m.body.start_selection();
        m.body.move_cursor(CursorMove::Forward);
        assert!(m.body.selection_range().is_some());

        let layout = edit_layout(TEST_AREA, false, 0);
        let inner = Block::bordered().inner(layout.body);
        let m = step_edit(m, click(inner.x + 3, inner.y));
        assert_eq!(
            m.body.selection_range(),
            None,
            "a plain click always clears any existing selection first"
        );
    }

    /// Esc layering (extends `esc_closes_dropdown_first_then_cancels`):
    /// selection cancels first, THEN dropdown, THEN the modal. Category can
    /// have both a selection and an open dropdown at once.
    #[test]
    fn esc_cancels_selection_before_dropdown_before_modal_on_category() {
        let mut m = EditModal::create(vec!["Work".into()]);
        m.category = TextArea::new(vec!["alpha beta".to_string()]);
        m.sync_blocks();
        m.category.start_selection();
        m.category.move_cursor(CursorMove::Forward);
        m.dropdown = Some(0);
        assert!(m.category.selection_range().is_some());

        let m = step_edit(m, key(KeyCode::Esc)); // 1st: selection only
        assert_eq!(m.category.selection_range(), None);
        assert_eq!(
            m.dropdown,
            Some(0),
            "dropdown untouched by the selection-cancel Esc"
        );

        let m = step_edit(m, key(KeyCode::Esc)); // 2nd: dropdown
        assert_eq!(m.dropdown, None);

        match step(Modal::Edit(m), key(KeyCode::Esc), TEST_AREA) {
            ModalStep::Continue(Modal::None) => {} // 3rd: modal cancels
            _ => panic!("third Esc must cancel the modal"),
        }
    }

    /// Same layering on Body, which has no dropdown at all: selection Esc,
    /// then straight to modal-cancel.
    #[test]
    fn esc_cancels_body_selection_before_cancelling_modal() {
        let mut m = create_modal();
        m.focus = EditFocus::Body;
        m.body = TextArea::new(vec!["hello".to_string()]);
        m.sync_blocks();
        m.body.start_selection();
        m.body.move_cursor(CursorMove::Forward);
        assert!(m.body.selection_range().is_some());

        let m = step_edit(m, key(KeyCode::Esc)); // 1st: selection
        assert_eq!(m.body.selection_range(), None);

        match step(Modal::Edit(m), key(KeyCode::Esc), TEST_AREA) {
            ModalStep::Continue(Modal::None) => {} // 2nd: modal cancels
            _ => panic!("second Esc must cancel the modal when there's no dropdown"),
        }
    }

    #[test]
    fn selection_style_is_set_on_all_three_fields() {
        let mut m = create_modal();
        assert_eq!(m.category.selection_style(), theme::selection());
        assert_eq!(m.title.selection_style(), theme::selection());
        assert_eq!(m.body.selection_style(), theme::selection());
    }
}
