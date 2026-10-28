//! PROTOTYPE — throwaway. Answers "Prototype: Today in the sidebar and edit
//! modal" (Adroz/herdr-feedr#17). Not wired into the real app; fake data,
//! palette copied from src/tui/theme.rs.
//!
//! Three variants of the Today section + the edit-modal Today toggle,
//! switchable live:
//!   1 / 2 / 3   variant A / B / C;  4 = D, the chosen mix (C outline, B sub-line + modal)
//!   s           cycle scenario: normal (2/3) · full (3/3) · over-cap (4/3, hand edit) · empty
//!   m           open/close the edit modal (on "Bump Node to 22", not in Today)
//!   t           press the modal's Today toggle (refused when full)
//!   [ / ]       sidebar width -/+ (20..46)
//!   q           quit
//!
//! Run: cargo run --example prototype_today

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
const SURFACE1: Color = Color::Rgb(0x45, 0x47, 0x5a);
const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
const OVERLAY0: Color = Color::Rgb(0x6c, 0x70, 0x86);
const OVERLAY1: Color = Color::Rgb(0x7f, 0x84, 0x9c);

#[derive(Clone)]
struct Item {
    state: char,
    title: &'static str,
    section: &'static str,
    today: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Normal,
    Full,
    Over,
    Empty,
}

struct Proto {
    variant: char,
    scenario: Scenario,
    width: u16,
    modal: bool,
    modal_today: bool,
    flash: Option<&'static str>,
}

fn items(s: Scenario) -> Vec<Item> {
    let mut v = vec![
        Item { state: ' ', title: "Fix auth redirect loop", section: "Work", today: true },
        Item { state: '~', title: "Migrate CI to GitHub Actions", section: "Work", today: true },
        Item { state: ' ', title: "Bump Node to 22", section: "Work", today: false },
        Item { state: ' ', title: "Evaluate pnpm catalogs", section: "Later", today: false },
        Item { state: ' ', title: "Write onboarding doc", section: "Later", today: false },
        Item { state: '?', title: "Add retry to flaky deploy test", section: "Agent", today: false },
    ];
    match s {
        Scenario::Normal => {}
        Scenario::Full => v[5].today = true,
        Scenario::Over => {
            v[5].today = true;
            v[3].today = true;
        }
        Scenario::Empty => v.iter_mut().for_each(|i| i.today = false),
    }
    v
}

fn glyph_style(c: char) -> Style {
    match c {
        '~' => Style::default().fg(YELLOW),
        '?' => Style::default().fg(MAUVE),
        _ => Style::default().fg(TEXT),
    }
}

fn ell(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn checkbox(i: &Item) -> Vec<Span<'static>> {
    vec![
        Span::styled("[", Style::default().fg(TEXT)),
        Span::styled(i.state.to_string(), glyph_style(i.state)),
        Span::styled("] ", Style::default().fg(TEXT)),
    ]
}

fn header(s: &str) -> Line<'static> {
    Line::styled(s.to_string(), Style::default().fg(LAVENDER).add_modifier(Modifier::BOLD))
}

fn muted(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::default().fg(OVERLAY0))
}

/// The ordinary sections, minus Today items (decided: Today items show only
/// in Today).
fn rest_of_list(lines: &mut Vec<Line<'static>>, it: &[Item], w: usize) {
    for sec in ["Work", "Later", "Agent"] {
        let rows: Vec<&Item> = it.iter().filter(|i| i.section == sec && !i.today).collect();
        if rows.is_empty() {
            continue;
        }
        lines.push(header(sec));
        for i in rows {
            let mut sp = checkbox(i);
            sp.push(Span::styled(ell(i.title, w - 4), Style::default().fg(TEXT)));
            lines.push(Line::from(sp));
        }
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(muted("+ add")));
}

fn count(it: &[Item]) -> usize {
    it.iter().filter(|i| i.today).count()
}

/// A — plain section header "Today 2/3", rows carry a dim right-aligned
/// category suffix. Reads exactly like the other sections.
fn sidebar_a(it: &[Item], w: usize) -> Vec<Line<'static>> {
    let mut l = vec![Line::from(muted("toolbar ⌕ 🧹 ▤")), Line::raw("")];
    let n = count(it);
    if n > 0 {
        let cap = if n > 3 {
            Span::styled(format!(" {n}/3 !"), Style::default().fg(RED))
        } else {
            muted(format!(" {n}/3"))
        };
        l.push(Line::from(vec![
            Span::styled("Today", Style::default().fg(PEACH).add_modifier(Modifier::BOLD)),
            cap,
        ]));
        for i in it.iter().filter(|i| i.today) {
            let suffix = format!(" {}", i.section);
            let title_w = w.saturating_sub(4 + suffix.chars().count());
            let t = ell(i.title, title_w);
            let pad = title_w.saturating_sub(t.chars().count());
            let mut sp = checkbox(i);
            sp.push(Span::styled(t, Style::default().fg(TEXT)));
            sp.push(Span::raw(" ".repeat(pad)));
            sp.push(muted(suffix));
            l.push(Line::from(sp));
        }
        l.push(Line::raw(""));
    }
    rest_of_list(&mut l, it, w);
    l
}

/// B — no header; Today rows get a peach gutter bar and a slot number,
/// category drops to an indented sub-line (like the @agent sub-line). Empty
/// slots are shown as "· free" so the cap is visible without a counter.
fn sidebar_b(it: &[Item], w: usize) -> Vec<Line<'static>> {
    let mut l = vec![Line::from(muted("toolbar ⌕ 🧹 ▤")), Line::raw("")];
    let today: Vec<&Item> = it.iter().filter(|i| i.today).collect();
    if !today.is_empty() {
        let bar = |c: Color| Span::styled("▌", Style::default().fg(c));
        for (n, i) in today.iter().enumerate() {
            let c = if n >= 3 { RED } else { PEACH };
            let mut sp = vec![bar(c), Span::styled(format!("{} ", n + 1), Style::default().fg(c))];
            sp.extend(checkbox(i));
            sp.push(Span::styled(ell(i.title, w - 7), Style::default().fg(TEXT)));
            l.push(Line::from(sp));
            l.push(Line::from(vec![bar(c), Span::styled(format!("    {}", i.section), Style::default().fg(OVERLAY1))]));
        }
        for n in today.len()..3 {
            l.push(Line::from(vec![bar(SURFACE1), muted(format!("{} · free", n + 1))]));
        }
        if today.len() > 3 {
            l.push(Line::from(Span::styled("  over cap — edited by hand", Style::default().fg(RED))));
        }
        l.push(Line::raw(""));
    }
    rest_of_list(&mut l, it, w);
    l
}

/// C — Today is a bordered panel pinned above the list (doesn't scroll
/// with it), counter in the border title, no category shown at all.
/// Rendered by the caller as a Block; this returns only the panel's rows.
fn sidebar_c_panel(it: &[Item], w: usize) -> Vec<Line<'static>> {
    it.iter()
        .filter(|i| i.today)
        .map(|i| {
            let mut sp = checkbox(i);
            sp.push(Span::styled(ell(i.title, w - 6), Style::default().fg(TEXT)));
            Line::from(sp)
        })
        .collect()
}

fn draw_sidebar(f: &mut Frame, area: Rect, p: &Proto, it: &[Item]) {
    f.render_widget(Block::default().style(Style::default().bg(BASE)), area);
    let w = area.width as usize;
    let status = Rect { y: area.y + area.height - 1, height: 1, ..area };
    let done = Rect { y: area.y + area.height - 3, height: 1, ..area };
    let body = Rect { height: area.height - 3, ..area };
    match p.variant {
        'a' => f.render_widget(Paragraph::new(sidebar_a(it, w)), body),
        'b' => f.render_widget(Paragraph::new(sidebar_b(it, w)), body),
        'd' => {
            // D (CHOSEN): C's outline, no count; category on a sub-line like
            // B; red outline when over cap.
            let n = count(it);
            let mut y = body.y;
            f.render_widget(Paragraph::new(Line::from(muted("toolbar ⌕ 🧹 ▤"))), Rect { y, height: 1, ..body });
            y += 2;
            if n > 0 {
                let mut rows = vec![];
                for i in it.iter().filter(|i| i.today) {
                    let mut sp = checkbox(i);
                    sp.push(Span::styled(ell(i.title, w - 6), Style::default().fg(TEXT)));
                    rows.push(Line::from(sp));
                    rows.push(Line::from(Span::styled(format!("    {}", i.section), Style::default().fg(OVERLAY1))));
                }
                let h = rows.len() as u16 + 2;
                let colour = if n > 3 { RED } else { PEACH };
                let block = Block::bordered()
                    .border_style(Style::default().fg(colour))
                    .title(Span::styled(" Today ", Style::default().fg(colour).add_modifier(Modifier::BOLD)));
                f.render_widget(Paragraph::new(rows).block(block), Rect { y, height: h, ..body });
                y += h + 1;
            }
            let mut rest = vec![];
            rest_of_list(&mut rest, it, w);
            f.render_widget(Paragraph::new(rest), Rect { y, height: body.height.saturating_sub(y - body.y), ..body });
        }
        _ => {
            let n = count(it);
            let mut y = body.y;
            f.render_widget(Paragraph::new(Line::from(muted("toolbar ⌕ 🧹 ▤"))), Rect { y, height: 1, ..body });
            y += 2;
            if n > 0 {
                let rows = sidebar_c_panel(it, w);
                let h = rows.len() as u16 + 2;
                let colour = if n > 3 { RED } else { PEACH };
                let block = Block::bordered()
                    .border_style(Style::default().fg(colour))
                    .title(Span::styled(format!(" Today {n}/3 "), Style::default().fg(colour).add_modifier(Modifier::BOLD)));
                f.render_widget(Paragraph::new(rows).block(block), Rect { y, height: h, ..body });
                y += h + 1;
            }
            let mut rest = vec![];
            rest_of_list(&mut rest, it, w);
            f.render_widget(Paragraph::new(rest), Rect { y, height: body.height.saturating_sub(y - body.y), ..body });
        }
    }
    f.render_widget(Paragraph::new(muted("Done (4)")), done);
    let msg = p.flash.unwrap_or("");
    f.render_widget(Paragraph::new(Line::from(vec![muted(ell(msg, w - 1))])), status);
}

fn field(f: &mut Frame, r: Rect, label: &str, text: &str, focused: bool) {
    let b = Block::bordered()
        .title(label.to_string())
        .border_style(Style::default().fg(if focused { LAVENDER } else { SURFACE1 }));
    f.render_widget(Paragraph::new(text.to_string()).style(Style::default().fg(TEXT)).wrap(Wrap { trim: false }).block(b), r);
}

fn button(f: &mut Frame, r: Rect, label: &str, on: bool, accent: Color) {
    let st = if on {
        Style::default().fg(BASE).bg(accent)
    } else {
        Style::default().fg(TEXT).bg(SURFACE1)
    };
    f.render_widget(Paragraph::new(label.to_string()).style(st), r);
}

/// Edit modal, zoomed over the whole sidebar like the real one.
fn draw_modal(f: &mut Frame, area: Rect, p: &Proto, full: bool) {
    f.render_widget(Clear, area);
    let mut title = vec![Span::styled("Edit item", Style::default().fg(LAVENDER).add_modifier(Modifier::BOLD))];
    if p.variant == 'b' || p.variant == 'd' {
        // B (and chosen D): toggle lives in the modal's top border, right of the title.
        let (g, c) = if p.modal_today { ("★ Today", PEACH) } else { ("☆ Today", OVERLAY1) };
        title.push(Span::raw(" ── "));
        title.push(Span::styled(g, Style::default().fg(c)));
    }
    let outer = Block::bordered()
        .style(Style::default().bg(MANTLE))
        .border_style(Style::default().fg(SURFACE1))
        .title(Line::from(title))
        .padding(Padding::uniform(1));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let x = inner.x;
    let w = inner.width;
    let mut y = inner.y;
    field(f, Rect { x, y, width: w, height: 3 }, "Category", "Work", false);
    y += 3;
    if p.variant == 'a' {
        // A: a checkbox row of its own, between Category and Title, in the
        // Tab order; Space or click toggles.
        let mark = if p.modal_today { "[x]" } else { "[ ]" };
        let used = if full && !p.modal_today { "  3/3 full" } else { "" };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {mark} "), Style::default().fg(PEACH)),
                Span::styled("Today", Style::default().fg(TEXT)),
                Span::styled(used, Style::default().fg(RED)),
            ])),
            Rect { x, y, width: w, height: 1 },
        );
        y += 2;
    }
    field(f, Rect { x, y, width: w, height: 3 }, "Title", "Bump Node to 22", true);
    y += 3;
    let body_h = inner.height.saturating_sub(y - inner.y + 4);
    field(f, Rect { x, y, width: w, height: body_h }, "Body", "Check .nvmrc and CI images.", false);
    y += body_h + 1;
    // Buttons. C: Today is a toggle button in the button row.
    let mut bx = x;
    for (label, on, accent) in [(" Save ", false, LAVENDER), (" Cancel ", false, LAVENDER)] {
        let bw = label.len() as u16;
        button(f, Rect { x: bx, y, width: bw, height: 1 }, label, on, accent);
        bx += bw + 1;
    }
    if p.variant == 'c' {
        let label = if p.modal_today { " ★ Today " } else { " ☆ Today " };
        let bw = label.chars().count() as u16;
        if bx + bw <= x + w {
            button(f, Rect { x: bx, y, width: bw, height: 1 }, label, p.modal_today, PEACH);
        } else {
            button(f, Rect { x, y: y + 1, width: bw, height: 1 }, label, p.modal_today, PEACH);
        }
    }
    let hint = match p.variant {
        'a' => "Tab field · Space toggle · ^S save",
        'b' | 'd' => "^T today · ^S save · Esc cancel",
        _ => "Tab field · ^S save · Esc cancel",
    };
    f.render_widget(Paragraph::new(muted(hint)), Rect { x, y: inner.y + inner.height - 1, width: w, height: 1 });
}

fn draw(f: &mut Frame, p: &Proto) {
    let full_area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(Color::Black)), full_area);
    let mut it = items(p.scenario);
    let full = count(&it) >= 3;
    if p.modal_today {
        it[2].today = true;
    }
    let sb = Rect { x: 0, y: 0, width: p.width.min(full_area.width), height: full_area.height.saturating_sub(4) };
    draw_sidebar(f, sb, p, &it);
    if p.modal {
        draw_modal(f, sb, p, full);
    }
    // Surface the state (prototype rule): what variant/scenario is live.
    let scen = match p.scenario {
        Scenario::Normal => "normal 2/3",
        Scenario::Full => "full 3/3",
        Scenario::Over => "over-cap 4/3 (hand edit)",
        Scenario::Empty => "empty 0/3",
    };
    let state = format!(
        "PROTOTYPE  variant {}  scenario {}  width {}  modal {}  modal-today {}",
        p.variant.to_ascii_uppercase(), scen, p.width, p.modal, p.modal_today
    );
    let keys = "1/2/3/4 variant · s scenario · m modal · t toggle Today · [ ] width · q quit";
    let bar = Rect { x: 0, y: full_area.height.saturating_sub(3), width: full_area.width, height: 3 };
    f.render_widget(
        Paragraph::new(vec![Line::styled(state, Style::default().fg(YELLOW)), Line::from(muted(keys))])
            .block(Block::default().style(Style::default().bg(Color::Black))),
        bar,
    );
}

fn main() -> std::io::Result<()> {
    let mut term = ratatui::init();
    let mut p = Proto { variant: 'a', scenario: Scenario::Normal, width: 30, modal: false, modal_today: false, flash: None };
    loop {
        term.draw(|f| draw(f, &p))?;
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            p.flash = None;
            match k.code {
                KeyCode::Char('q') => break,
                KeyCode::Char(c @ ('1' | '2' | '3' | '4')) => p.variant = (b'a' + (c as u8 - b'1')) as char,
                KeyCode::Char('s') => {
                    p.scenario = match p.scenario {
                        Scenario::Normal => Scenario::Full,
                        Scenario::Full => Scenario::Over,
                        Scenario::Over => Scenario::Empty,
                        Scenario::Empty => Scenario::Normal,
                    };
                    p.modal_today = false;
                }
                KeyCode::Char('m') => p.modal = !p.modal,
                KeyCode::Char('t') if p.modal => {
                    let full = count(&items(p.scenario)) >= 3;
                    if !p.modal_today && full {
                        p.flash = Some("Today full — unmark one first");
                    } else {
                        p.modal_today = !p.modal_today;
                    }
                }
                KeyCode::Char('[') => p.width = p.width.saturating_sub(2).max(20),
                KeyCode::Char(']') => p.width = (p.width + 2).min(46),
                _ => {}
            }
        }
    }
    ratatui::restore();
    Ok(())
}
