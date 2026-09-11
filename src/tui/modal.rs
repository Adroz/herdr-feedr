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

use crate::feed::{Document, Item};
use crate::tui::app::{section_choices, ItemKey, SectionChoice};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::widgets::Block;
use tui_textarea::{CursorMove, TextArea};

pub enum Modal {
    None,
    Edit(EditModal),
    ConfirmDelete(EditModal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFocus {
    Section,
    Title,
    Body,
}

pub struct EditModal {
    /// `Some(key)` = editing an existing item; `None` = creating.
    pub original: Option<ItemKey>,
    pub choices: Vec<SectionChoice>,
    pub choice_idx: usize,
    pub title: TextArea<'static>,
    pub body: TextArea<'static>,
    pub focus: EditFocus,
}

impl EditModal {
    pub fn create(doc: &Document) -> Self {
        let mut m = EditModal {
            original: None,
            choices: section_choices(doc),
            choice_idx: 0,
            title: TextArea::default(),
            body: TextArea::default(),
            focus: EditFocus::Section,
        };
        m.sync_blocks();
        m
    }

    pub fn edit(key: ItemKey, item: &Item) -> Self {
        let mut m = EditModal {
            original: Some(key),
            choices: Vec::new(), // moving sections is $EDITOR territory (spec §3)
            choice_idx: 0,
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

    pub fn choice_label(&self) -> String {
        match &self.choices[self.choice_idx] {
            SectionChoice::FirstHuman => "(first section)".into(),
            SectionChoice::Named(s) => s.clone(),
            SectionChoice::Agent => "Agent".into(),
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match (self.focus, self.original.is_some()) {
            (EditFocus::Section, _) => EditFocus::Title,
            (EditFocus::Title, _) => EditFocus::Body,
            (EditFocus::Body, true) => EditFocus::Title, // no section field on edit
            (EditFocus::Body, false) => EditFocus::Section,
        };
        self.sync_blocks();
    }

    /// Mark the focused field's border title with `*` so focus is visible.
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

/// Result of feeding one terminal event to the active modal. `Continue`
/// covers every transition that stays purely in-memory (including
/// cancelling to `Modal::None`); `Save` and `Delete` are the two points
/// where the caller must go through `with_feed` to touch the file.
pub enum ModalStep {
    Continue(Modal),
    Save(EditModal),
    Delete(ItemKey),
}

/// Pure modal transition: `(modal, event) -> ModalStep`. Never touches the
/// filesystem — mirrors `input::translate`.
pub fn step(modal: Modal, ev: Event) -> ModalStep {
    // Ignore key releases (Windows terminals emit them).
    if let Event::Key(k) = &ev {
        if k.kind == KeyEventKind::Release {
            return ModalStep::Continue(modal);
        }
    }
    match modal {
        Modal::None => ModalStep::Continue(Modal::None),
        Modal::Edit(m) => edit_step(m, ev),
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
    }
}

fn edit_step(mut m: EditModal, ev: Event) -> ModalStep {
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
            _ => {}
        }
    }
    match m.focus {
        EditFocus::Section => {
            if let Event::Key(k) = &ev {
                match k.code {
                    KeyCode::Left => {
                        m.choice_idx = (m.choice_idx + m.choices.len() - 1) % m.choices.len();
                    }
                    KeyCode::Right | KeyCode::Char(' ') => {
                        m.choice_idx = (m.choice_idx + 1) % m.choices.len();
                    }
                    _ => {}
                }
            }
        }
        EditFocus::Title => {
            m.title.input(ev);
        }
        EditFocus::Body => {
            m.body.input(ev);
        }
    }
    ModalStep::Continue(Modal::Edit(m))
}
