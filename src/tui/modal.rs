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

pub struct EditModal {
    /// `Some(key)` = editing an existing item; `None` = creating.
    pub original: Option<ItemKey>,
    pub title: TextArea<'static>,
    pub body: TextArea<'static>,
    pub focus: EditFocus,
}

impl EditModal {
    /// Sidebar-created items are always the human's, and always land at the
    /// end of the first human section (spec §3 review, round 2 item 5) —
    /// agents create their own items via the CLI into `## Agent`, and can
    /// relocate items later by editing the feed. So there is no section
    /// picker to focus first; Title is focused immediately.
    pub fn create() -> Self {
        let mut m = EditModal {
            original: None,
            title: TextArea::default(),
            body: TextArea::default(),
            focus: EditFocus::Title,
        };
        m.sync_blocks();
        m
    }

    pub fn edit(key: ItemKey, item: &Item) -> Self {
        let mut m = EditModal {
            original: Some(key),
            title: TextArea::new(vec![item.title.clone()]),
            body: TextArea::new(item.body.clone()),
            focus: EditFocus::Title,
        };
        // TextArea::new leaves the cursor at (0,0); appending is the common
        // edit, so park it at the end deterministically.
        m.title.move_cursor(CursorMove::End);
        m.body.move_cursor(CursorMove::Bottom);
        m.body.move_cursor(CursorMove::End);
        m.sync_blocks();
        m
    }

    /// Tab order: Title → Body → Save → Cancel → (Delete →) back to the
    /// start. Create mode has no item to delete.
    pub fn cycle_focus(&mut self) {
        let is_edit = self.original.is_some();
        self.focus = match (self.focus, is_edit) {
            (EditFocus::Title, _) => EditFocus::Body,
            (EditFocus::Body, _) => EditFocus::Save,
            (EditFocus::Save, _) => EditFocus::Cancel,
            (EditFocus::Cancel, true) => EditFocus::Delete,
            (EditFocus::Cancel, false) => EditFocus::Title,
            (EditFocus::Delete, _) => EditFocus::Title,
        };
        self.sync_blocks();
    }

    /// Mark the focused field's border title with `*` so focus is visible,
    /// and neutralize both textareas' own cursor styling — tui-textarea
    /// renders a reverse-video cell at its cursor position regardless of
    /// focus, but the only cursor that should ever be visible is the real
    /// terminal cursor the focused field gets from `frame.set_cursor_position`
    /// (round-2 feedback item 1).
    pub fn sync_blocks(&mut self) {
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
        self.title.set_block(Block::bordered().title(t));
        self.body.set_block(Block::bordered().title(b));
        self.title.set_cursor_style(Style::default());
        self.body.set_cursor_style(Style::default());
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
pub fn edit_layout(term_area: Rect, is_edit: bool) -> EditLayout {
    let outer = centered(term_area, 90, 80);
    let inner = Block::bordered().padding(Padding::uniform(1)).inner(outer);
    let [title, body, buttons, hints] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

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
            (KeyCode::Esc, _) => return ModalStep::Continue(Modal::None),
            (KeyCode::Tab, _) => {
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
            let layout = edit_layout(area, is_edit);
            return match edit_click(&layout, is_edit, mev.column, mev.row) {
                Some(EditClickTarget::Title) => {
                    m.focus = EditFocus::Title;
                    m.sync_blocks();
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Body) => {
                    m.focus = EditFocus::Body;
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

    fn create_modal() -> EditModal {
        EditModal::create()
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
        assert!(edit_layout(TEST_AREA, false).delete.is_none());
        assert!(edit_layout(TEST_AREA, true).delete.is_some());
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
    #[test]
    fn edit_layout_buttons_never_escape_the_terminal_at_any_width() {
        for width in 1..=120u16 {
            let area = Rect::new(0, 0, width, 40);
            for is_edit in [false, true] {
                let layout = edit_layout(area, is_edit);
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
            }
        }
    }

    /// Round-2 item 5: the section picker is gone, so create mode focuses
    /// Title immediately (no Section field to click into or through).
    #[test]
    fn clicking_title_or_body_moves_focus_there() {
        let layout = edit_layout(TEST_AREA, false);
        let m = create_modal();
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
        let layout = edit_layout(TEST_AREA, false);
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
        let layout = edit_layout(TEST_AREA, false);
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
        let layout = edit_layout(TEST_AREA, true);
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
        // Create mode: Title -> Body -> Save -> Cancel -> Title.
        let mut m = create_modal();
        for _ in 0..2 {
            m.cycle_focus();
        }
        assert_eq!(m.focus, EditFocus::Save);
        assert!(matches!(
            step(Modal::Edit(m), enter(), TEST_AREA),
            ModalStep::Save(_)
        ));

        let mut m = create_modal();
        for _ in 0..3 {
            m.cycle_focus();
        }
        assert_eq!(m.focus, EditFocus::Cancel);
        assert!(matches!(
            step(Modal::Edit(m), enter(), TEST_AREA),
            ModalStep::Continue(Modal::None)
        ));

        // Edit mode: Title -> Body -> Save -> Cancel -> Delete.
        let mut m = edit_modal();
        for _ in 0..4 {
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
    /// via `frame.set_cursor_position` on the focused field. Both
    /// textareas' internal cursor style must be neutralized at all times —
    /// on construction and after every focus change.
    #[test]
    fn textareas_never_render_their_own_cursor_style() {
        let m = create_modal();
        assert_eq!(m.title.cursor_style(), Style::default());
        assert_eq!(m.body.cursor_style(), Style::default());

        let mut m = edit_modal();
        assert_eq!(m.title.cursor_style(), Style::default());
        assert_eq!(m.body.cursor_style(), Style::default());
        m.cycle_focus(); // Title -> Body
        assert_eq!(m.title.cursor_style(), Style::default());
        assert_eq!(m.body.cursor_style(), Style::default());
    }
}
