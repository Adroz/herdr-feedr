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
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
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
    #[allow(dead_code)] // wired up in Task 10 (save path)
    pub original_category: Option<String>,
    pub category: TextArea<'static>,
    pub title: TextArea<'static>,
    pub body: TextArea<'static>,
    /// Category suggestions captured at open (`ops::section_names`).
    pub suggestions: Vec<String>,
    /// Keyboard highlight into `filtered()`; None = not in the list.
    pub dropdown: Option<usize>,
    pub focus: EditFocus,
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
        let mut m = EditModal {
            original: Some(key),
            category: TextArea::new(vec![section.clone().unwrap_or_default()]),
            original_category: section,
            title: TextArea::new(vec![item.title.clone()]),
            body: TextArea::new(item.body.clone()),
            suggestions,
            dropdown: None,
            focus: EditFocus::Category,
        };
        // TextArea::new leaves the cursor at (0,0); appending is the common
        // edit, so park it at the end deterministically.
        m.category.move_cursor(CursorMove::End);
        m.title.move_cursor(CursorMove::End);
        m.body.move_cursor(CursorMove::Bottom);
        m.body.move_cursor(CursorMove::End);
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

    /// Mark the focused field's border title with `*` so focus is visible,
    /// and neutralize the textareas' own cursor styling — tui-textarea
    /// renders a reverse-video cell at its cursor position regardless of
    /// focus, but the only cursor that should ever be visible is the real
    /// terminal cursor the focused field gets from `frame.set_cursor_position`
    /// (round-2 feedback item 1).
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
        self.category.set_cursor_style(Style::default());
        self.title.set_cursor_style(Style::default());
        self.body.set_cursor_style(Style::default());
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

    /// Replace the field content with an accepted suggestion.
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
    /// Zero-height when the dropdown is closed. Overlays the title/body
    /// area; `edit_click` tests it first so overlap resolves to the list.
    pub dropdown: Rect,
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
    // Inside the category field's borders, clamped like the buttons so it
    // can never escape the drawn buffer.
    let dropdown = Rect::new(
        category.x.saturating_add(1),
        category.y.saturating_add(category.height),
        category.width.saturating_sub(2),
        dropdown_rows,
    )
    .intersection(term_area);

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
        title,
        body,
        hints,
        save,
        cancel,
        delete,
    }
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
        return Some(EditClickTarget::Suggestion(y - layout.dropdown.y));
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

fn edit_step(mut m: EditModal, ev: Event, area: Rect) -> ModalStep {
    if let Event::Key(k) = &ev {
        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                if m.focus == EditFocus::Category && m.dropdown.is_some() {
                    m.dropdown = None;
                    return ModalStep::Continue(Modal::Edit(m));
                }
                return ModalStep::Continue(Modal::None);
            }
            (KeyCode::Tab, _) => {
                m.dropdown = None;
                m.cycle_focus();
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
            (KeyCode::Down, _) if m.focus == EditFocus::Category => {
                // Clamp against the ROWS ACTUALLY DRAWN, not just the cap: at
                // a degenerate terminal size the dropdown rect is clipped by
                // `.intersection(term_area)`, and the highlight must never
                // point at a row the user cannot see (Enter would accept an
                // invisible suggestion).
                let visible = edit_layout(area, m.original.is_some(), m.dropdown_rows())
                    .dropdown
                    .height as usize;
                let n = m.filtered().len().min(MAX_DROPDOWN_ROWS).min(visible);
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
                if let Some(i) = m.dropdown {
                    m.accept_suggestion(i);
                } else {
                    m.focus = EditFocus::Title;
                    m.sync_blocks();
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Enter, _) if m.focus == EditFocus::Title => {
                m.focus = EditFocus::Body;
                m.sync_blocks();
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
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Title) => {
                    m.focus = EditFocus::Title;
                    m.dropdown = None;
                    m.sync_blocks();
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Body) => {
                    m.focus = EditFocus::Body;
                    m.dropdown = None;
                    m.sync_blocks();
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Save) => ModalStep::Save(m),
                Some(EditClickTarget::Cancel) => ModalStep::Continue(Modal::None),
                Some(EditClickTarget::Delete) => ModalStep::Continue(Modal::ConfirmDelete(m)),
                None => ModalStep::Continue(Modal::Edit(m)),
            };
        }
    }
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
    fn edit_layout_buttons_never_escape_the_terminal_at_any_width() {
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

    /// Round-2 item 1: tui-textarea renders its own reverse-video cursor
    /// cell regardless of focus; the only cursor that should ever be
    /// visible is the real terminal cursor `view::draw_edit_modal` places
    /// via `frame.set_cursor_position` on the focused field. All three
    /// textareas' internal cursor style must be neutralized at all times —
    /// on construction and after every focus change.
    #[test]
    fn textareas_never_render_their_own_cursor_style() {
        let m = create_modal();
        assert_eq!(m.category.cursor_style(), Style::default());
        assert_eq!(m.title.cursor_style(), Style::default());
        assert_eq!(m.body.cursor_style(), Style::default());

        let mut m = edit_modal();
        assert_eq!(m.category.cursor_style(), Style::default());
        assert_eq!(m.title.cursor_style(), Style::default());
        assert_eq!(m.body.cursor_style(), Style::default());
        m.cycle_focus(); // Category -> Title
        assert_eq!(m.category.cursor_style(), Style::default());
        assert_eq!(m.title.cursor_style(), Style::default());
        assert_eq!(m.body.cursor_style(), Style::default());
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
        assert_eq!(l.dropdown.height, 3);
        assert_eq!(l.dropdown.y, l.category.y + l.category.height);
        let l0 = edit_layout(TEST_AREA, false, 0);
        assert_eq!(l0.dropdown.height, 0);
    }

    #[test]
    fn clicks_hit_category_and_dropdown_rows() {
        let l = edit_layout(TEST_AREA, false, 2);
        assert_eq!(
            edit_click(&l, false, l.category.x + 1, l.category.y + 1),
            Some(EditClickTarget::Category)
        );
        // The dropdown overlays the title area — it must win the hit-test:
        assert_eq!(
            edit_click(&l, false, l.dropdown.x + 1, l.dropdown.y + 1),
            Some(EditClickTarget::Suggestion(1))
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
        assert_eq!(m.focus, EditFocus::Category);
        let m = EditModal::edit(key, &item, None, vec![]);
        assert_eq!(m.category_text(), "");
        assert_eq!(m.original_category, None);
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
        let visible = edit_layout(tiny, false, m.dropdown_rows()).dropdown.height as usize;
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

    /// B3 review follow-up: a click that moves focus off Category (to Title
    /// or Body) must also clear a stale dropdown highlight — Tab already did
    /// this, but a focus-moving click didn't.
    #[test]
    fn clicking_title_or_body_clears_stale_dropdown_highlight() {
        let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
        let m = step_edit(m, key(KeyCode::Down)); // dropdown open
        assert_eq!(m.dropdown, Some(0));
        let layout = edit_layout(TEST_AREA, false, m.dropdown_rows());
        let m = step_edit(m, click(layout.title.x, layout.title.y));
        assert_eq!(m.focus, EditFocus::Title);
        assert_eq!(m.dropdown, None);
    }

    #[test]
    fn click_on_suggestion_accepts_it() {
        let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
        let m = step_edit(m, key(KeyCode::Down)); // dropdown open (2 rows)
        let l = edit_layout(TEST_AREA, false, m.dropdown_rows());
        let m = step_edit(m, click(l.dropdown.x + 1, l.dropdown.y + 1));
        assert_eq!(m.category_text(), "Chores");
        assert_eq!(m.dropdown, None);
    }
}
