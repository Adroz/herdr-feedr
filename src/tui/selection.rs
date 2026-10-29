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
use std::time::{Duration, Instant};
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

/// What a triple-click selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triple {
    /// The logical line under the pointer (a body line, a viewer line).
    Line,
    /// The whole text (the one-line Title and Category fields).
    Field,
}

/// A left press that is click number `clicks` of a run (from
/// [`ClickTracker`]): [`press`], then, for a double, select the word there,
/// and for a triple, what `triple` says. Like a drag, the selection is
/// copied on release, so the press itself copies nothing.
pub fn press_n(ta: &mut TextArea<'_>, x: u16, y: u16, clicks: u8, triple: Triple) {
    let Some(pos) = press(ta, x, y) else {
        return;
    };
    match (clicks, triple) {
        (2, _) => {
            select_word(ta, pos);
        }
        (3, Triple::Line) => select_line(ta, pos.0),
        (3, Triple::Field) => select_all(ta),
        _ => {}
    }
}

/// Double-click: select the word ([`word_at`]) around `pos`. Returns false,
/// leaving the field as it was, when there's no word there (a separator, or
/// past the end of the line).
fn select_word(ta: &mut TextArea<'_>, (line, ch): TextPos) -> bool {
    let Some(range) = ta.lines().get(line).and_then(|l| word_at(l, ch)) else {
        return false;
    };
    select_span(ta, (line, range.start), (line, range.end));
    true
}

/// Triple-click in a multi-line field: select the whole logical line `line`
/// (every soft-wrapped row of it, without the line break).
fn select_line(ta: &mut TextArea<'_>, line: usize) {
    let Some(len) = ta.lines().get(line).map(|l| l.chars().count()) else {
        return;
    };
    select_span(ta, (line, 0), (line, len));
}

/// Triple-click in a one-line field: select the whole field.
fn select_all(ta: &mut TextArea<'_>) {
    let last = ta.lines().len().saturating_sub(1);
    let len = ta.lines().last().map_or(0, |l| l.chars().count());
    select_span(ta, (0, 0), (last, len));
}

fn select_span(ta: &mut TextArea<'_>, from: TextPos, to: TextPos) {
    ta.cancel_selection();
    jump(ta, from);
    ta.start_selection();
    jump(ta, to);
}

fn jump(ta: &mut TextArea<'_>, (line, ch): TextPos) {
    let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    ta.move_cursor(CursorMove::Jump(clamp(line), clamp(ch)));
}

// --- Multi-click ------------------------------------------------------------

/// How soon after the previous press a press still counts toward a double or
/// triple click (herdr v0.9.3 `ClientPaneClick::is_double_click_for`).
pub const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(350);

/// Counts consecutive left presses into single, double and triple clicks,
/// with herdr's rule: a press continues the run when it comes within
/// [`MULTI_CLICK_WINDOW`] of the previous press and within one cell of it on
/// each axis. A press after a triple starts a new run. The time is passed in,
/// so the owner feeds it from its injected clock.
#[derive(Debug, Default)]
pub struct ClickTracker {
    last: Option<LastPress>,
}

#[derive(Debug, Clone, Copy)]
struct LastPress {
    at: Instant,
    x: u16,
    y: u16,
    count: u8,
}

impl ClickTracker {
    /// Record a press at screen cell `(x, y)` and return its place in the
    /// run: 1 (single), 2 (double) or 3 (triple). `plain` is false when a
    /// modifier was held: like herdr, such a press is always a single and
    /// starts no run.
    pub fn press(&mut self, at: Instant, x: u16, y: u16, plain: bool) -> u8 {
        if !plain {
            self.last = None;
            return 1;
        }
        let count = match self.last {
            Some(p)
                if p.count < 3
                    && at.saturating_duration_since(p.at) <= MULTI_CLICK_WINDOW
                    && p.x.abs_diff(x) <= 1
                    && p.y.abs_diff(y) <= 1 =>
            {
                p.count + 1
            }
            _ => 1,
        };
        self.last = Some(LastPress { at, x, y, count });
        count
    }
}

// --- Word bounds ------------------------------------------------------------
//
// A port of herdr v0.9.3's double-click word bounds (`word_bounds_at_column`
// and helpers in herdr's `src/app/actions.rs`), kept structurally close so it
// can be diffed against upstream: a whole http(s) URL, else a quoted path,
// else a separator-delimited token with wrapping punctuation trimmed.
//
// herdr maps a terminal row to display cells first so the clicked column
// lands on the right char. Here [`pos_at_cell`] has already done that
// against the text as drawn (wide chars, wrap and scroll included), so the
// bounds work in char indices.

/// The char-index range (end exclusive) of the word a double-click on the
/// char at `idx` selects in `line`, or `None` on a separator, a lone quote,
/// or past the end of the line.
pub fn word_at(line: &str, idx: usize) -> Option<std::ops::Range<usize>> {
    let cells: Vec<char> = line.chars().collect();
    if idx >= cells.len() {
        return None;
    }
    let span = url_span(&cells, idx)
        .or_else(|| quoted_path_span(&cells, idx))
        .or_else(|| token_span(&cells, idx))?;
    Some(span.start..span.end + 1)
}

/// Inclusive char-index span.
#[derive(Debug, Clone, Copy)]
struct CellSpan {
    start: usize,
    end: usize,
}

impl CellSpan {
    fn contains(self, idx: usize) -> bool {
        idx >= self.start && idx <= self.end
    }
}

fn token_span(cells: &[char], clicked: usize) -> Option<CellSpan> {
    if is_word_separator(cells[clicked]) {
        return None;
    }
    let mut start = clicked;
    while start > 0 && !is_word_separator(cells[start - 1]) {
        start -= 1;
    }
    let mut end = clicked;
    while end + 1 < cells.len() && !is_word_separator(cells[end + 1]) {
        end += 1;
    }
    trim_token_edges(cells, CellSpan { start, end }).filter(|s| s.contains(clicked))
}

fn starts_with_chars(cells: &[char], prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(i, expected)| cells.get(i) == Some(&expected))
}

fn url_span(cells: &[char], clicked: usize) -> Option<CellSpan> {
    let mut start = 0;
    while start < cells.len() {
        if starts_with_chars(&cells[start..], "http://")
            || starts_with_chars(&cells[start..], "https://")
        {
            let mut end = start;
            while end + 1 < cells.len() && !cells[end + 1].is_whitespace() {
                end += 1;
            }
            if clicked >= start && clicked <= end {
                let span = trim_url_edges(cells, CellSpan { start, end })?;
                return span.contains(clicked).then_some(span);
            }
            start = end + 1;
        } else {
            start += 1;
        }
    }
    None
}

fn trim_url_edges(cells: &[char], span: CellSpan) -> Option<CellSpan> {
    let start = span.start;
    let mut end = span.end;
    while start <= end && should_trim_trailing_url_cell(cells, start, end) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn should_trim_trailing_url_cell(cells: &[char], start: usize, end: usize) -> bool {
    match cells[end] {
        '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?' => true,
        ')' => !closer_is_balanced(cells, start, end, '(', ')'),
        ']' => !closer_is_balanced(cells, start, end, '[', ']'),
        '}' => !closer_is_balanced(cells, start, end, '{', '}'),
        _ => false,
    }
}

/// Whether the closer at `end` closes an opener inside `start..end`.
fn closer_is_balanced(cells: &[char], start: usize, end: usize, open: char, close: char) -> bool {
    let mut balance = 0i32;
    for &c in &cells[start..end] {
        if c == open {
            balance += 1;
        } else if c == close {
            balance -= 1;
        }
    }
    balance > 0
}

/// The inside of the quoted string around the click, if it contains a `/`
/// (so it's a path, possibly with spaces). A click on a quote itself never
/// matches.
fn quoted_path_span(cells: &[char], clicked: usize) -> Option<CellSpan> {
    let ch = *cells.get(clicked)?;
    if matches!(ch, '"' | '\'' | '`') {
        return None;
    }
    for quote in ['"', '\'', '`'] {
        let mut open = None;
        for (idx, &cell) in cells.iter().enumerate() {
            if cell != quote || is_escaped(cells, idx) {
                continue;
            }
            match open {
                Some(o) => {
                    if clicked > o && clicked < idx && cells[o + 1..idx].contains(&'/') {
                        return Some(CellSpan {
                            start: o + 1,
                            end: idx - 1,
                        });
                    }
                    open = None;
                }
                None => open = Some(idx),
            }
        }
    }
    None
}

fn is_escaped(cells: &[char], idx: usize) -> bool {
    let mut slashes = 0;
    let mut i = idx;
    while i > 0 && cells[i - 1] == '\\' {
        slashes += 1;
        i -= 1;
    }
    slashes % 2 == 1
}

fn is_word_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '|' | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | '!'
                | '（'
                | '）'
                | '：'
                | '、'
                | '。'
                | '，'
        )
}

fn trim_token_edges(cells: &[char], span: CellSpan) -> Option<CellSpan> {
    let mut start = span.start;
    let mut end = span.end;
    while start <= end && is_leading_token_wrapper(cells[start]) {
        start += 1;
    }
    // Keep a shell-prompt-style `$` that follows a closer: "( master)$".
    if start < end && cells[end] == '$' && is_trailing_token_wrapper(cells[end - 1]) {
        end -= 1;
    }
    while start <= end && is_trailing_token_wrapper(cells[end]) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn is_leading_token_wrapper(ch: char) -> bool {
    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'' | '`')
}

fn is_trailing_token_wrapper(ch: char) -> bool {
    matches!(
        ch,
        ')' | ']' | '}' | '>' | '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    // --- Multi-click tracker ---------------------------------------------

    #[test]
    fn a_second_click_within_350ms_is_a_double_and_351ms_is_a_single() {
        let t0 = Instant::now();
        let mut c = ClickTracker::default();
        assert_eq!(c.press(t0, 10, 5, true), 1);
        assert_eq!(c.press(t0 + ms(349), 10, 5, true), 2);

        let mut c = ClickTracker::default();
        c.press(t0, 10, 5, true);
        assert_eq!(c.press(t0 + ms(351), 10, 5, true), 1);
    }

    #[test]
    fn a_double_tolerates_one_cell_of_movement_in_each_direction() {
        let t0 = Instant::now();
        for (x, y) in [(9, 4), (11, 6), (9, 6), (11, 4), (10, 5)] {
            let mut c = ClickTracker::default();
            c.press(t0, 10, 5, true);
            assert_eq!(c.press(t0 + ms(100), x, y, true), 2, "({x},{y})");
        }
        for (x, y) in [(8, 5), (12, 5), (10, 3), (10, 7)] {
            let mut c = ClickTracker::default();
            c.press(t0, 10, 5, true);
            assert_eq!(c.press(t0 + ms(100), x, y, true), 1, "({x},{y})");
        }
    }

    #[test]
    fn a_third_quick_click_is_a_triple_and_a_fourth_starts_over() {
        let t0 = Instant::now();
        let mut c = ClickTracker::default();
        c.press(t0, 10, 5, true);
        c.press(t0 + ms(300), 10, 5, true);
        // Each click is timed and placed against the one before it.
        assert_eq!(c.press(t0 + ms(600), 11, 5, true), 3);
        assert_eq!(c.press(t0 + ms(700), 11, 5, true), 1);
        assert_eq!(c.press(t0 + ms(800), 11, 5, true), 2);
    }

    #[test]
    fn a_modified_click_is_always_single_and_breaks_the_run() {
        // herdr only counts unmodified clicks toward a double.
        let t0 = Instant::now();
        let mut c = ClickTracker::default();
        c.press(t0, 10, 5, true);
        assert_eq!(c.press(t0 + ms(50), 10, 5, false), 1);
        assert_eq!(c.press(t0 + ms(100), 10, 5, true), 1);
    }

    // --- Word bounds (cases ported from herdr v0.9.3 app/actions.rs) -------

    /// The text a double-click on the char `needle` starts at selects.
    fn word_by_char(row: &str, needle: &str) -> Option<String> {
        let byte = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} in {row:?}"));
        let idx = row[..byte].chars().count();
        let r = word_at(row, idx)?;
        Some(row.chars().skip(r.start).take(r.len()).collect())
    }

    #[test]
    fn double_click_word_bounds_match_herdr() {
        let cases = [
            // URLs: whole, trailing punctuation trimmed, balanced closers kept.
            (
                "see https://example.com/a-b_c?q=x@y.",
                "example.com",
                "https://example.com/a-b_c?q=x@y",
            ),
            (
                "open \"https://example.com/a,b;c?q=x\";",
                "example.com",
                "https://example.com/a,b;c?q=x",
            ),
            (
                "see https://en.wikipedia.org/wiki/Foo_(bar_(baz)),",
                "wikipedia",
                "https://en.wikipedia.org/wiki/Foo_(bar_(baz))",
            ),
            (
                "see https://example.com/a(b[c{d}e]f),",
                "example.com",
                "https://example.com/a(b[c{d}e]f)",
            ),
            (
                "see (https://example.com/a(b(c)d)))",
                "example.com",
                "https://example.com/a(b(c)d)",
            ),
            // Paths and tokens.
            (
                "open /tmp/foo-bar/baz_qux/",
                "foo-bar",
                "/tmp/foo-bar/baz_qux/",
            ),
            (
                "open ./src/app/actions.rs:795",
                "actions",
                "./src/app/actions.rs:795",
            ),
            (
                "open ../herdr-worktrees/issue-1",
                "herdr",
                "../herdr-worktrees/issue-1",
            ),
            (
                "edit src/app/actions.rs,then",
                "actions",
                "src/app/actions.rs",
            ),
            // Quoted paths keep their spaces.
            (
                "cat \"/tmp/build output/log.txt\"",
                "output",
                "/tmp/build output/log.txt",
            ),
            (
                "cat '/Users/me/Library/Application Support/app/config.json'",
                "Support",
                "/Users/me/Library/Application Support/app/config.json",
            ),
            // Wide characters.
            ("echo 你好-world done", "好", "你好-world"),
            (
                "註解已補（slice 4 5b5fcc0715）：整合原本",
                "5b5fcc0715",
                "5b5fcc0715",
            ),
            ("先跑 cargo test", "cargo", "cargo"),
            // Plain tokens.
            (
                "export PATH=$HOME/.cargo/bin:$PATH",
                "$HOME",
                "PATH=$HOME/.cargo/bin:$PATH",
            ),
            (
                "git checkout feature/foo-bar_baz",
                "foo",
                "feature/foo-bar_baz",
            ),
            ("refs #123 and @owner/name", "#123", "#123"),
            ("refs #123 and @owner/name", "owner", "@owner/name"),
            ("cargo test --package=herdr", "--package", "--package=herdr"),
            (
                "cargo test app::actions::tests",
                "app::",
                "app::actions::tests",
            ),
            (
                "image ghcr.io/org/app:latest",
                "ghcr",
                "ghcr.io/org/app:latest",
            ),
            ("ERROR [worker-1] request_id=abc-123", "worker", "worker-1"),
            (
                "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
                "newhoo",
                "newhoo",
            ),
            (
                "render_status_line(app, area)",
                "render",
                "render_status_line",
            ),
            ("render_status_line(app, area)", "app", "app"),
            ("render_status_line(app, area)", "area", "area"),
            ("if !enabled {", "enabled", "enabled"),
            ("println!(\"hi\")", "println", "println"),
            ("( master)$", "master", "master"),
            ("regex foo$", "foo", "foo$"),
        ];
        for (row, click, expected) in cases {
            assert_eq!(
                word_by_char(row, click).as_deref(),
                Some(expected),
                "row={row:?} click={click:?}"
            );
        }
    }

    #[test]
    fn separators_and_cjk_punctuation_select_nothing() {
        for delim in ['（', '）', '：', '、', '。', '，'] {
            let row = format!("left{delim}right");
            assert_eq!(word_by_char(&row, "left").as_deref(), Some("left"));
            assert_eq!(word_by_char(&row, "right").as_deref(), Some("right"));
            assert_eq!(word_at(&row, 4), None, "{delim:?}");
        }
        for (row, click) in [("a|b", "|"), ("one two", " "), ("f(x)", "(")] {
            assert_eq!(word_by_char(row, click), None, "{row:?} at {click:?}");
        }
    }

    #[test]
    fn a_click_past_the_end_of_the_line_selects_nothing() {
        assert_eq!(word_at("abc", 3), None);
        assert_eq!(word_at("", 0), None);
    }
}
