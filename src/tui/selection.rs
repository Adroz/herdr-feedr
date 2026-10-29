//! Pointer → text mapping for selectable text surfaces (spec #22).
//!
//! Every click and drag on a text field goes through here, so the cursor and
//! selection endpoints land on the character drawn under the pointer however
//! the text is laid out: soft-wrapped continuation rows, a scrolled viewport,
//! and wide (CJK / emoji) characters that take two cells.
//!
//! Positions are logical [`TextPos`] values, `(line, char)` with `char` a
//! char index into that line (not a byte offset or display column), which is
//! the coordinate space `TextArea::cursor`, `selection_range` and
//! `CursorMove::Jump` all use.
//!
//! The mapping is resolved against the field *as last drawn*: that is the
//! layout the user was looking at when they clicked. tui-textarea records its
//! inner area and viewport on every render and exposes the inverse mapping as
//! `TextArea::cursor_at_position`, so this module never re-derives wrap or
//! scroll itself and can't drift from what was rendered.

use ratatui::layout::Position;
use tui_textarea::{CursorMove, TextArea};

/// A logical position in a text: `(line, char index within the line)`.
pub type TextPos = (usize, usize);

/// The logical position under screen cell `(x, y)`, as the field was last
/// drawn. `None` when the cell is outside the field's text area (its border,
/// or anywhere else on screen). Cells past the end of a row clamp to that
/// row's last cursor position; the right half of a wide character maps to
/// that character.
pub fn pos_at_cell(ta: &TextArea<'_>, x: u16, y: u16) -> Option<TextPos> {
    ta.cursor_at_position(Position::new(x, y))
}

/// Mouse-down: drop any selection and put the cursor on the character under
/// `(x, y)`. Returns the new cursor position, or `None` (cursor untouched,
/// selection still dropped) when the cell holds no text position.
pub fn press(ta: &mut TextArea<'_>, x: u16, y: u16) -> Option<TextPos> {
    // Clear first: `Jump` extends an active selection rather than replacing it.
    ta.cancel_selection();
    let pos = pos_at_cell(ta, x, y)?;
    jump(ta, pos);
    Some(pos)
}

/// Mouse-drag: extend the selection from its anchor to the character under
/// `(x, y)`. With no non-empty selection yet, the anchor is the current
/// cursor, which the preceding [`press`] placed. Cells with no text position
/// leave the selection as it is and return `None`.
pub fn drag_to(ta: &mut TextArea<'_>, x: u16, y: u16) -> Option<TextPos> {
    let pos = pos_at_cell(ta, x, y)?;
    if ta.selection_range().is_none() {
        // (Re)anchor here: a zero-length selection left over from a
        // same-cell drag frame is not yet "extending".
        ta.start_selection();
    }
    jump(ta, pos);
    Some(pos)
}

/// The selected text, lines joined by `\n`. `None` when nothing is selected,
/// including a zero-length selection (anchor and cursor on the same spot).
pub fn selected_text(ta: &TextArea<'_>) -> Option<String> {
    let ((sl, sc), (el, ec)) = ta.selection_range()?;
    if (sl, sc) == (el, ec) {
        return None;
    }
    let lines = ta.lines();
    let slice = |l: usize, from: usize, to: usize| -> String {
        lines[l]
            .chars()
            .skip(from)
            .take(to.saturating_sub(from))
            .collect()
    };
    if sl == el {
        return Some(slice(sl, sc, ec));
    }
    let mut out = slice(sl, sc, usize::MAX);
    for line in &lines[sl + 1..el] {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.push_str(&slice(el, 0, ec));
    Some(out)
}

/// Copy: the selected text, also put in the field's own yank buffer (so
/// Ctrl+V pastes it). Unlike `TextArea::copy`, the selection stays
/// highlighted: it goes on the next click or key, not on copying.
pub fn copy(ta: &mut TextArea<'_>) -> Option<String> {
    let text = selected_text(ta)?;
    ta.set_yank_text(text.clone());
    Some(text)
}

fn jump(ta: &mut TextArea<'_>, (line, ch): TextPos) {
    let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    ta.move_cursor(CursorMove::Jump(clamp(line), clamp(ch)));
}
