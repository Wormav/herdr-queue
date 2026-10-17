use crate::discovery::{discover_sessions, load_agents};
use crate::models::{Agent, Composer, Draft, Error, Result, Target};
use crate::service::DraftService;
use crate::store::DraftStore;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::collections::HashMap;
use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tui_textarea::{CursorMove, TextArea, WrapMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Save,
    SendNext,
    Quit,
    New,
    Agents,
    Refresh,
    Newline,
}

fn shortcut(key: KeyEvent) -> Option<Shortcut> {
    if key.modifiers.contains(KeyModifiers::SUPER) {
        return match key.code {
            KeyCode::Char('u' | 'U') => Some(Shortcut::Save),
            KeyCode::Char('y' | 'Y') => Some(Shortcut::SendNext),
            KeyCode::Char('l' | 'L') => Some(Shortcut::Quit),
            KeyCode::Char('p' | 'P') => Some(Shortcut::New),
            KeyCode::Char('o' | 'O') => Some(Shortcut::Agents),
            KeyCode::Char('r' | 'R') => Some(Shortcut::Refresh),
            _ => None,
        };
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('s') => Some(Shortcut::Save),
            KeyCode::Char('t') => Some(Shortcut::SendNext),
            KeyCode::Char('q' | 'c') => Some(Shortcut::Quit),
            KeyCode::Char('n') => Some(Shortcut::New),
            KeyCode::Char('g') => Some(Shortcut::Agents),
            KeyCode::Char('r') => Some(Shortcut::Refresh),
            KeyCode::Char('j') => Some(Shortcut::Newline),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => Some(Shortcut::Newline),
        KeyCode::Enter | KeyCode::F(2) => Some(Shortcut::Save),
        KeyCode::F(8) => Some(Shortcut::SendNext),
        KeyCode::F(10) => Some(Shortcut::Quit),
        KeyCode::F(4) => Some(Shortcut::New),
        KeyCode::F(3) => Some(Shortcut::Agents),
        KeyCode::F(5) => Some(Shortcut::Refresh),
        _ => None,
    }
}

fn next_send<'a>(
    drafts: &'a [Draft],
    editing: Option<&str>,
    history: bool,
    confirming: bool,
    busy: bool,
) -> Option<&'a str> {
    if history || confirming || busy {
        return None;
    }
    drafts
        .first()
        .filter(|draft| {
            Some(draft.id.as_str()) != editing && matches!(draft.state.as_str(), "draft" | "error")
        })
        .map(|draft| draft.id.as_str())
}

fn edit_input(editor: &mut TextArea<'_>, event: Event) -> bool {
    match event {
        Event::Paste(text) => {
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            if text.chars().any(terminal_control) {
                return false;
            }
            let mut changed = false;
            for (index, line) in text.split('\n').enumerate() {
                if index > 0 {
                    editor.insert_newline();
                    changed = true;
                }
                changed |= editor.insert_str(line);
            }
            changed
        }
        Event::Key(key) => match shortcut(key) {
            Some(Shortcut::Newline) => {
                editor.insert_newline();
                true
            }
            Some(_) => false,
            None => editor.input(key),
        },
        _ => false,
    }
}

fn terminal_control(character: char) -> bool {
    (character < ' ' && character != '\n' && character != '\t')
        || ('\u{7f}'..='\u{9f}').contains(&character)
}

fn display_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if terminal_control(character) {
                '�'
            } else {
                character
            }
        })
        .collect()
}

fn place_caret(editor: &mut TextArea<'_>, area: Rect, column: u16, row: u16) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if !inner.contains((column, row).into()) {
        return;
    }
    let (top_row, left_column) = editor.scroll_offset();
    let clicked = editor.screen_to_data(
        usize::from(top_row) + usize::from(row - inner.y),
        usize::from(left_column) + usize::from(column - inner.x),
    );
    editor.cancel_selection();
    editor.move_cursor(CursorMove::Jump(
        clicked.0.min(u16::MAX as usize) as u16,
        clicked.1.min(u16::MAX as usize) as u16,
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueueAction {
    Save,
    Send(String),
    New,
    History,
    Edit(String),
    Reuse(String),
    Move(String, i32),
    Delete(String),
    Resolve(String, bool),
    Confirm(bool),
    Refresh,
    Quit,
    Agents,
    Select(usize),
}

struct QueueView {
    editor: TextArea<'static>,
    drafts: Vec<Draft>,
    editing_id: Option<String>,
    history: bool,
    busy: bool,
    confirming: Option<QueueAction>,
    allow_switch: bool,
    dirty: bool,
    selected: usize,
    scroll: usize,
    editor_focus: bool,
    status: String,
    notice: String,
    hits: Vec<(Rect, QueueAction)>,
    editor_area: Rect,
    list_area: Rect,
    confirm_yes: bool,
    focus_button: Option<QueueAction>,
}

impl QueueView {
    fn new(composer: Composer, drafts: Vec<Draft>, allow_switch: bool) -> Self {
        let mut editor = TextArea::from(
            display_text(&composer.body)
                .split('\n')
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        );
        editor.set_wrap_mode(WrapMode::WordOrGlyph);
        Self {
            editor,
            drafts,
            editing_id: composer.editing_id,
            history: false,
            busy: false,
            confirming: None,
            allow_switch,
            dirty: true,
            selected: 0,
            scroll: 0,
            editor_focus: true,
            status: "Connexion…".into(),
            notice: String::new(),
            hits: Vec::new(),
            editor_area: Rect::default(),
            list_area: Rect::default(),
            confirm_yes: false,
            focus_button: None,
        }
    }

    fn body(&self) -> String {
        self.editor.lines().join("\n")
    }

    fn set_composer(&mut self, body: &str, editing_id: Option<String>) {
        self.editor = TextArea::from(
            display_text(body)
                .split('\n')
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        );
        self.editor.set_wrap_mode(WrapMode::WordOrGlyph);
        self.editing_id = editing_id;
        self.editor_focus = true;
        self.focus_button = None;
        self.dirty = true;
    }

    fn notify(&mut self, notice: impl Into<String>) {
        let notice = notice.into();
        if self.notice != notice {
            self.notice = notice;
            self.dirty = true;
        }
    }

    fn select(&mut self, offset: isize) {
        let selected = self
            .selected
            .saturating_add_signed(offset)
            .min(self.drafts.len().saturating_sub(1));
        self.dirty |= self.selected != selected || self.editor_focus;
        self.selected = selected;
        self.editor_focus = false;
        self.focus_button = None;
        if self.selected < self.scroll || offset > 0 {
            self.scroll = self.selected;
        }
    }

    fn tab(&mut self, backwards: bool) {
        let actions = self
            .hits
            .iter()
            .filter_map(|(_, action)| {
                if matches!(action, QueueAction::Select(_)) {
                    None
                } else {
                    Some(action.clone())
                }
            })
            .collect::<Vec<_>>();
        let save = actions
            .iter()
            .position(|action| matches!(action, QueueAction::Save))
            .unwrap_or(usize::from(self.allow_switch).min(actions.len()));
        let mut order = actions;
        order.insert(save, QueueAction::Select(usize::MAX));
        let current = if self.editor_focus {
            save
        } else {
            self.focus_button
                .as_ref()
                .and_then(|action| order.iter().position(|candidate| candidate == action))
                .unwrap_or(save)
        };
        let next = if backwards {
            current.checked_sub(1).unwrap_or(order.len() - 1)
        } else {
            (current + 1) % order.len()
        };
        self.editor_focus = next == save;
        self.focus_button = if self.editor_focus {
            None
        } else {
            Some(order[next].clone())
        };
        self.dirty = true;
    }

    fn event(&mut self, event: Event) -> Option<QueueAction> {
        match event {
            Event::Resize(_, _) => {
                self.dirty = true;
                None
            }
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if let Some((_, action)) = self
                        .hits
                        .iter()
                        .rev()
                        .find(|(area, _)| area.contains((mouse.column, mouse.row).into()))
                    {
                        return Some(action.clone());
                    }
                    if self.confirming.is_none()
                        && self.editor_area.contains((mouse.column, mouse.row).into())
                    {
                        self.dirty = true;
                        self.editor_focus = true;
                        self.focus_button = None;
                        place_caret(&mut self.editor, self.editor_area, mouse.column, mouse.row);
                    }
                } else if self.confirming.is_none()
                    && self.editor_area.contains((mouse.column, mouse.row).into())
                {
                    if matches!(
                        mouse.kind,
                        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                    ) {
                        self.editor.input(Event::Mouse(mouse));
                        self.dirty = true;
                    }
                } else if self.confirming.is_none()
                    && self.list_area.contains((mouse.column, mouse.row).into())
                {
                    match mouse.kind {
                        MouseEventKind::ScrollDown => self.select(1),
                        MouseEventKind::ScrollUp => self.select(-1),
                        _ => {}
                    }
                }
                None
            }
            Event::Key(key) if key.kind == KeyEventKind::Release => None,
            Event::Key(key) if self.confirming.is_some() => match key.code {
                KeyCode::Esc => Some(QueueAction::Confirm(false)),
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    self.confirm_yes = !self.confirm_yes;
                    self.dirty = true;
                    None
                }
                KeyCode::Enter => Some(QueueAction::Confirm(self.confirm_yes)),
                _ => None,
            },
            Event::Key(key) => {
                if key.code == KeyCode::Enter && !self.editor_focus && key.modifiers.is_empty() {
                    if let Some(action) = &self.focus_button {
                        return Some(action.clone());
                    }
                }
                if let Some(command) = shortcut(key) {
                    let action = match command {
                        Shortcut::Save if key.code != KeyCode::Enter || self.editor_focus => {
                            Some(QueueAction::Save)
                        }
                        Shortcut::Save => self.drafts.get(self.selected).and_then(|draft| {
                            match draft.state.as_str() {
                                "draft" | "error"
                                    if !self.busy
                                        && self.editing_id.as_deref()
                                            != Some(draft.id.as_str()) =>
                                {
                                    Some(QueueAction::Send(draft.id.clone()))
                                }
                                "sent" => Some(QueueAction::Reuse(draft.id.clone())),
                                "uncertain" => Some(QueueAction::Resolve(draft.id.clone(), false)),
                                _ => None,
                            }
                        }),
                        Shortcut::SendNext => next_send(
                            &self.drafts,
                            self.editing_id.as_deref(),
                            self.history,
                            self.confirming.is_some(),
                            self.busy,
                        )
                        .map(|id| QueueAction::Send(id.into())),
                        Shortcut::New => Some(QueueAction::New),
                        Shortcut::Agents if self.allow_switch => Some(QueueAction::Agents),
                        Shortcut::Agents => None,
                        Shortcut::Refresh => Some(QueueAction::Refresh),
                        Shortcut::Quit => Some(QueueAction::Quit),
                        Shortcut::Newline if self.editor_focus => {
                            self.editor.insert_newline();
                            self.dirty = true;
                            None
                        }
                        Shortcut::Newline => None,
                    };
                    return action;
                }
                match key.code {
                    KeyCode::Tab | KeyCode::BackTab => {
                        self.tab(
                            key.code == KeyCode::BackTab
                                || key.modifiers.contains(KeyModifiers::SHIFT),
                        );
                        None
                    }
                    KeyCode::Esc if !self.editor_focus => {
                        self.editor_focus = true;
                        self.focus_button = None;
                        self.dirty = true;
                        None
                    }
                    KeyCode::Down if !self.editor_focus => {
                        self.select(1);
                        None
                    }
                    KeyCode::Up if !self.editor_focus => {
                        self.select(-1);
                        None
                    }
                    KeyCode::PageDown if !self.editor_focus => {
                        self.select(3);
                        None
                    }
                    KeyCode::PageUp if !self.editor_focus => {
                        self.select(-3);
                        None
                    }
                    KeyCode::Char('h') if !self.editor_focus => Some(QueueAction::History),
                    KeyCode::Char('e') if !self.editor_focus => self
                        .drafts
                        .get(self.selected)
                        .map(|draft| QueueAction::Edit(draft.id.clone())),
                    KeyCode::Delete if !self.editor_focus => self
                        .drafts
                        .get(self.selected)
                        .map(|draft| QueueAction::Delete(draft.id.clone())),
                    _ if self.editor_focus => {
                        let before = self.editor.cursor();
                        self.dirty |= edit_input(&mut self.editor, Event::Key(key))
                            || before != self.editor.cursor();
                        None
                    }
                    _ => None,
                }
            }
            Event::Paste(text) if self.confirming.is_none() && self.editor_focus => {
                if text
                    .chars()
                    .any(|character| terminal_control(character) && character != '\r')
                {
                    self.notify("Collage refusé : caractère de contrôle du terminal.");
                    return None;
                }
                self.dirty |= edit_input(&mut self.editor, Event::Paste(text));
                None
            }
            _ => None,
        }
    }
}

fn native_style() -> Style {
    Style::default().fg(Color::Reset).bg(Color::Reset)
}

fn status_label(status: &str) -> &str {
    match status {
        "working" => "En cours",
        "idle" | "done" => "Disponible",
        "blocked" => "Réponse attendue dans le terminal",
        _ => "État inconnu",
    }
}

fn draft_label(draft: &Draft, editing: bool) -> &str {
    if editing {
        return "En cours de modification";
    }
    match draft.state.as_str() {
        "draft" => "Prêt",
        "sending" => "Envoi en cours…",
        "sent" => "Transmis au terminal",
        "error" => "À réessayer",
        "uncertain" => "Envoi à vérifier",
        _ => "État inconnu",
    }
}

fn draw_button(
    frame: &mut Frame,
    hits: &mut Vec<(Rect, QueueAction)>,
    area: Rect,
    label: &str,
    action: QueueAction,
    enabled: bool,
    selected: bool,
) {
    if area.is_empty() {
        return;
    }
    let style = if selected {
        native_style().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else if enabled {
        native_style().fg(Color::Cyan)
    } else {
        native_style().fg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new(format!("[{label}]")).style(style), area);
    if enabled {
        hits.push((area, action));
    }
}

fn card_actions(
    draft: &Draft,
    editing: bool,
    busy: bool,
) -> Vec<(&'static str, QueueAction, bool)> {
    let id = &draft.id;
    match draft.state.as_str() {
        "draft" | "error" => vec![
            ("Envoyer", QueueAction::Send(id.clone()), !busy && !editing),
            ("Modifier", QueueAction::Edit(id.clone()), !busy && !editing),
            ("↑", QueueAction::Move(id.clone(), -1), !busy),
            ("↓", QueueAction::Move(id.clone(), 1), !busy),
            (
                "Supprimer",
                QueueAction::Delete(id.clone()),
                !busy && !editing,
            ),
        ],
        "uncertain" => vec![
            (
                "Remettre en brouillon",
                QueueAction::Resolve(id.clone(), false),
                !busy,
            ),
            (
                "Déjà transmis",
                QueueAction::Resolve(id.clone(), true),
                !busy,
            ),
        ],
        "sent" => vec![("Reprendre", QueueAction::Reuse(id.clone()), !busy)],
        _ => Vec::new(),
    }
}

fn button_rows(actions: &[(&str, QueueAction, bool)], width: u16) -> u16 {
    let mut rows = 1;
    let mut x = 0;
    for (label, _, _) in actions {
        let size = (Line::from(*label).width() as u16 + 2).min(width);
        if x > 0 && x + size > width {
            rows += 1;
            x = 0;
        }
        x = x.saturating_add(size + 1);
    }
    if actions.is_empty() {
        0
    } else {
        rows
    }
}

fn draw_actions(
    frame: &mut Frame,
    hits: &mut Vec<(Rect, QueueAction)>,
    area: Rect,
    actions: &[(&str, QueueAction, bool)],
    focused: Option<&QueueAction>,
) {
    let mut x = 0;
    let mut y = 0;
    for (label, action, enabled) in actions {
        let size = (Line::from(*label).width() as u16 + 2).min(area.width);
        if x > 0 && x + size > area.width {
            y += 1;
            x = 0;
        }
        if y >= area.height {
            break;
        }
        draw_button(
            frame,
            hits,
            Rect::new(area.x + x, area.y + y, size, 1),
            label,
            action.clone(),
            *enabled,
            focused == Some(action),
        );
        x = x.saturating_add(size + 1);
    }
}

fn confirmation_text(action: &QueueAction) -> &'static str {
    match action {
        QueueAction::Delete(_) => "Supprimer ce brouillon ?",
        QueueAction::Resolve(_, false) => "Vérifiez le terminal : ce message n'a pas été transmis ?\nConfirmer le remet en brouillon, sans l'envoyer.",
        QueueAction::Resolve(_, true) => "Vous avez retrouvé ce message dans le terminal ?\nConfirmer le classe dans l'historique.",
        _ => "Effacer le texte en cours pour préparer un nouveau message ?",
    }
}

fn draw_queue(frame: &mut Frame, view: &mut QueueView, target: &Target) {
    let area = frame.area();
    frame.render_widget(Block::default().style(native_style()), area);
    view.hits.clear();
    if area.width < 20 || area.height < 13 {
        frame.render_widget(
            Paragraph::new("Brouillons\nAgrandissez le terminal.\n⌘L / Ctrl+Q : fermer")
                .style(native_style())
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let editor_height = if area.height < 28 { 5 } else { 8 };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(editor_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new("Brouillons").style(native_style().add_modifier(Modifier::BOLD)),
        chunks[0],
    );
    let destination = if view.allow_switch {
        Rect::new(
            chunks[1].x,
            chunks[1].y,
            chunks[1].width.saturating_sub(9),
            1,
        )
    } else {
        chunks[1]
    };
    frame.render_widget(
        Paragraph::new(display_text(&format!(
            "{} / {}",
            target.agent, target.pane_id
        )))
        .style(native_style()),
        destination,
    );
    if view.allow_switch {
        draw_button(
            frame,
            &mut view.hits,
            Rect::new(chunks[1].right().saturating_sub(8), chunks[1].y, 8, 1),
            "Agents",
            QueueAction::Agents,
            !view.busy,
            view.focus_button.as_ref() == Some(&QueueAction::Agents),
        );
    }
    frame.render_widget(
        Paragraph::new(display_text(&target.cwd)).style(native_style().fg(Color::DarkGray)),
        chunks[2],
    );
    frame.render_widget(
        Paragraph::new(display_text(&view.status)).style(native_style().fg(Color::Yellow)),
        chunks[3],
    );
    let title = if view.editing_id.is_some() {
        "Modifier le message"
    } else {
        "Écrire un message"
    };
    view.editor.set_block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(native_style().fg(if view.editor_focus {
                Color::Cyan
            } else {
                Color::DarkGray
            })),
    );
    view.editor.set_style(native_style());
    view.editor.set_cursor_line_style(native_style());
    view.editor.set_cursor_style(if view.editor_focus {
        native_style().add_modifier(Modifier::REVERSED)
    } else {
        native_style()
    });
    view.editor
        .set_placeholder_text("Préparez votre prochain message…");
    view.editor
        .set_placeholder_style(native_style().fg(Color::DarkGray));
    view.editor_area = chunks[4];
    frame.render_widget(&view.editor, chunks[4]);
    let can_save = !view.body().trim().is_empty();
    draw_actions(
        frame,
        &mut view.hits,
        chunks[5],
        &[
            (
                if view.editing_id.is_some() {
                    "Enregistrer"
                } else {
                    "Ajouter"
                },
                QueueAction::Save,
                can_save,
            ),
            ("Nouveau", QueueAction::New, true),
        ],
        view.focus_button.as_ref(),
    );
    let hint = if view.body().is_empty() {
        "Entrée : ajouter · Maj+Entrée : ligne".into()
    } else {
        format!("{} / 65 536 octets · sauvegardé", view.body().len())
    };
    frame.render_widget(
        Paragraph::new(hint).style(native_style().fg(Color::DarkGray)),
        chunks[6],
    );
    let title = format!(
        "{} ({})",
        if view.history {
            "Transmis"
        } else {
            "À envoyer"
        },
        view.drafts.len()
    );
    frame.render_widget(
        Paragraph::new(title).style(native_style().add_modifier(Modifier::BOLD)),
        chunks[7],
    );
    let history_label = if view.history {
        "Brouillons"
    } else {
        "Historique"
    };
    let width = history_label.len() as u16 + 2;
    draw_button(
        frame,
        &mut view.hits,
        Rect::new(
            chunks[7].right().saturating_sub(width),
            chunks[7].y,
            width,
            1,
        ),
        history_label,
        QueueAction::History,
        true,
        view.focus_button.as_ref() == Some(&QueueAction::History),
    );
    view.list_area = chunks[8];
    view.scroll = view.scroll.min(view.drafts.len().saturating_sub(1));
    let mut y = chunks[8].y;
    if view.drafts.is_empty() {
        frame.render_widget(
            Paragraph::new(if view.history {
                "Les messages transmis apparaîtront ici."
            } else {
                "Votre liste est vide.\nÉcrivez un message, puis ajoutez-le."
            })
            .style(native_style().fg(Color::DarkGray))
            .wrap(Wrap { trim: false }),
            chunks[8],
        );
    }
    for (index, draft) in view.drafts.iter().enumerate().skip(view.scroll) {
        let remaining = chunks[8].bottom().saturating_sub(y);
        if remaining < 3 {
            break;
        }
        let editing = view.editing_id.as_deref() == Some(draft.id.as_str());
        let actions = card_actions(draft, editing, view.busy);
        let inner_width = chunks[8].width.saturating_sub(2);
        let rows = button_rows(&actions, inner_width);
        let preview_lines = draft.body.lines().count().clamp(1, 4) as u16;
        let error_lines = u16::from(draft.error.is_some());
        let height = (2 + preview_lines + rows + error_lines).min(remaining);
        let card = Rect::new(chunks[8].x, y, chunks[8].width, height);
        view.hits.push((card, QueueAction::Select(index)));
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{}  {}", index + 1, draft_label(draft, editing)))
                .border_style(
                    native_style().fg(if !view.editor_focus && view.selected == index {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    }),
                ),
            card,
        );
        let available_preview = height.saturating_sub(2 + rows + error_lines);
        let preview = display_text(&draft.body)
            .lines()
            .take(4)
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(360)
            .collect::<String>();
        frame.render_widget(
            Paragraph::new(preview)
                .style(native_style())
                .wrap(Wrap { trim: false }),
            Rect::new(card.x + 1, y + 1, inner_width, available_preview),
        );
        if let Some(error) = &draft.error {
            frame.render_widget(
                Paragraph::new(display_text(error)).style(native_style().fg(Color::Yellow)),
                Rect::new(
                    card.x + 1,
                    y + 1 + available_preview,
                    inner_width,
                    error_lines,
                ),
            );
        }
        draw_actions(
            frame,
            &mut view.hits,
            Rect::new(
                card.x + 1,
                y + height.saturating_sub(rows + 1),
                inner_width,
                rows.min(height.saturating_sub(2)),
            ),
            &actions,
            view.focus_button.as_ref(),
        );
        y += height;
    }
    frame.render_widget(
        Paragraph::new(display_text(&view.notice)).style(native_style().fg(Color::Yellow)),
        chunks[9],
    );
    let footer = if cfg!(target_os = "macos") {
        "⌘U Ajouter  ⌘Y Suivant  ⌘L Fermer\nTab : liste · ↑↓ · Entrée · e : modifier"
    } else {
        "F2 Ajouter  F8 Suivant  F10 Fermer\nTab : liste · ↑↓ · Entrée · e : modifier"
    };
    frame.render_widget(
        Paragraph::new(footer).style(native_style().fg(Color::DarkGray)),
        chunks[10],
    );
    if let Some(action) = &view.confirming {
        let popup_width = area.width.saturating_sub(4).min(66);
        let popup_height = 8.min(area.height);
        let popup = Rect::new(
            area.x + (area.width - popup_width) / 2,
            area.y + (area.height - popup_height) / 2,
            popup_width,
            popup_height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("Confirmation")
                .style(native_style())
                .border_style(native_style().fg(Color::Yellow)),
            popup,
        );
        frame.render_widget(
            Paragraph::new(confirmation_text(action))
                .style(native_style())
                .wrap(Wrap { trim: false }),
            Rect::new(
                popup.x + 1,
                popup.y + 1,
                popup.width.saturating_sub(2),
                popup.height.saturating_sub(3),
            ),
        );
        view.hits.clear();
        draw_button(
            frame,
            &mut view.hits,
            Rect::new(popup.x + 1, popup.bottom().saturating_sub(2), 9, 1),
            "Annuler",
            QueueAction::Confirm(false),
            true,
            !view.confirm_yes,
        );
        draw_button(
            frame,
            &mut view.hits,
            Rect::new(
                popup.x + 11,
                popup.bottom().saturating_sub(2),
                popup.width.saturating_sub(12).min(11),
                1,
            ),
            "Confirmer",
            QueueAction::Confirm(true),
            true,
            view.confirm_yes,
        );
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupt_terminal(_signal: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

fn interrupted_error() -> Error {
    Error {
        message:
            "Interface interrompue. Les textes sont conservés ; vérifiez tout envoi interrompu."
                .into(),
        code: "interrupted".into(),
        uncertain: false,
    }
}

fn check_interruption() -> Result<()> {
    if INTERRUPTED.load(Ordering::Relaxed) {
        Err(interrupted_error())
    } else {
        Ok(())
    }
}

fn read_terminal_event() -> Result<Option<Event>> {
    check_interruption()?;
    let result = (|| -> io::Result<Option<Event>> {
        if event::poll(Duration::from_millis(100))? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    })();
    check_interruption()?;
    result.map_err(Error::from)
}

struct UiWorker<T> {
    receiver: Receiver<T>,
    thread: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

impl<T: Send + 'static> UiWorker<T> {
    fn spawn(
        name: &str,
        operation: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let reply = operation(&cancellation);
                let _ = sender.send(reply);
            })?;
        Ok(Self {
            receiver,
            thread: Some(thread),
            cancelled,
        })
    }
}

impl<T> Drop for UiWorker<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    );
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    previous_sigint: libc::sighandler_t,
    previous_sigterm: libc::sighandler_t,
}

impl TerminalSession {
    fn open() -> Result<Self> {
        check_interruption()?;
        static PANIC_HOOK: Once = Once::new();
        PANIC_HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                previous(info);
            }));
        });
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        enable_raw_mode()?;
        let session = Self {
            terminal,
            // The handlers only set an atomic flag; terminal restoration runs on the UI thread.
            previous_sigint: unsafe {
                libc::signal(
                    libc::SIGINT,
                    interrupt_terminal as *const () as libc::sighandler_t,
                )
            },
            previous_sigterm: unsafe {
                libc::signal(
                    libc::SIGTERM,
                    interrupt_terminal as *const () as libc::sighandler_t,
                )
            },
        };
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            cursor::Hide,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )?;
        Ok(session)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore_terminal();
        unsafe {
            libc::signal(libc::SIGINT, self.previous_sigint);
            libc::signal(libc::SIGTERM, self.previous_sigterm);
        }
    }
}

pub fn run_queue(
    service: DraftService,
    panel_id: Option<String>,
    allow_switch: bool,
) -> Result<bool> {
    check_interruption()?;
    let mut app = QueueApp::new(service, allow_switch)?;
    let mut session = TerminalSession::open()?;
    let mut worker = Some(queue_worker(&app.service, panel_id.clone(), None)?);
    let mut pending_send = None;
    let mut next_refresh = Instant::now() + Duration::from_secs(3);
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            let mut error = interrupted_error();
            if let Err(save_error) = app.persist() {
                error
                    .message
                    .push_str(&format!(" Sauvegarde finale impossible : {save_error}."));
            }
            return Err(error);
        }
        if let Some(receiver) = &worker {
            match receiver.receiver.try_recv() {
                Ok(reply) => {
                    worker = None;
                    app.reply(reply)?;
                    next_refresh = Instant::now() + Duration::from_secs(3);
                    if let Some(id) = pending_send.take() {
                        check_interruption()?;
                        worker = Some(queue_worker(&app.service, None, Some(id))?);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.service.store.recover_inflight(&app.service.target)?;
                    return Err(Error::new(
                        "La requête a été interrompue. Vérifiez le terminal avant de réessayer.",
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if worker.is_none() && !app.view.busy && Instant::now() >= next_refresh {
            check_interruption()?;
            worker = Some(queue_worker(&app.service, panel_id.clone(), None)?);
            next_refresh = Instant::now() + Duration::from_secs(3);
        }
        if app.view.dirty {
            session
                .terminal
                .draw(|frame| draw_queue(frame, &mut app.view, &app.service.target))?;
            app.view.dirty = false;
        }
        let Some(event) = read_terminal_event()? else {
            continue;
        };
        let before = app.view.body();
        let action = app.view.event(event);
        if app.view.body() != before {
            if let Err(error) = app.persist() {
                app.view.notify(format!(
                    "Sauvegarde impossible : {error}. Gardez ce panneau ouvert."
                ));
            }
        }
        if let Some(action) = action {
            check_interruption()?;
            match app.action(action) {
                Ok(QueueOutcome::Continue) => {}
                Ok(QueueOutcome::Close(switch)) => return Ok(switch),
                Ok(QueueOutcome::Refresh) if worker.is_none() => {
                    worker = Some(queue_worker(&app.service, panel_id.clone(), None)?);
                }
                Ok(QueueOutcome::Refresh) => {}
                Ok(QueueOutcome::Send(id)) if worker.is_some() => pending_send = Some(id),
                Ok(QueueOutcome::Send(id)) => {
                    worker = Some(queue_worker(&app.service, None, Some(id))?)
                }
                Err(error) => app.view.notify(error.to_string()),
            }
        }
    }
}

fn inventory_worker(
    env: HashMap<String, String>,
    session: Option<String>,
    socket: Option<String>,
) -> Result<UiWorker<(Vec<Agent>, Vec<String>)>> {
    UiWorker::spawn(
        "herdr-agent-inventory",
        move |cancelled| match discover_sessions(&env, session.as_deref(), socket.as_deref()) {
            Ok(sessions) => {
                let mut agents = Vec::new();
                let mut errors = Vec::new();
                for session in &sessions {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let (mut inventory, mut failures) = load_agents(std::slice::from_ref(session));
                    agents.append(&mut inventory);
                    errors.append(&mut failures);
                }
                (agents, errors)
            }
            Err(error) => (Vec::new(), vec![error.to_string()]),
        },
    )
}

pub fn pick_agent(
    env: &HashMap<String, String>,
    session: Option<&str>,
    socket: Option<&str>,
    notice: &str,
) -> Result<Option<Agent>> {
    let mut view = PickerView::new(notice);
    let mut terminal = TerminalSession::open()?;
    let mut worker = Some(inventory_worker(
        env.clone(),
        session.map(str::to_owned),
        socket.map(str::to_owned),
    )?);
    view.loading = true;
    let mut next_refresh = Instant::now() + Duration::from_secs(5);
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Err(interrupted_error());
        }
        if let Some(receiver) = &worker {
            match receiver.receiver.try_recv() {
                Ok((agents, errors)) => {
                    worker = None;
                    view.inventory(agents, errors);
                    next_refresh = Instant::now() + Duration::from_secs(5);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    worker = None;
                    view.inventory(
                        Vec::new(),
                        vec!["Recherche interrompue. Actualisez pour réessayer.".into()],
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if worker.is_none() && Instant::now() >= next_refresh {
            check_interruption()?;
            worker = Some(inventory_worker(
                env.clone(),
                session.map(str::to_owned),
                socket.map(str::to_owned),
            )?);
            view.loading = true;
            view.dirty = true;
            next_refresh = Instant::now() + Duration::from_secs(5);
        }
        if view.dirty {
            terminal
                .terminal
                .draw(|frame| draw_picker(frame, &mut view))?;
            view.dirty = false;
        }
        if let Some(event) = read_terminal_event()? {
            check_interruption()?;
            match view.event(event) {
                Some(PickerAction::Quit) => return Ok(None),
                Some(PickerAction::Choose(index)) => return Ok(view.agents.get(index).cloned()),
                Some(PickerAction::Refresh) if worker.is_none() => {
                    worker = Some(inventory_worker(
                        env.clone(),
                        session.map(str::to_owned),
                        socket.map(str::to_owned),
                    )?);
                    view.loading = true;
                    view.dirty = true;
                }
                _ => {}
            }
        }
    }
}

struct QueueApp {
    service: DraftService,
    view: QueueView,
}

#[derive(Debug, PartialEq, Eq)]
enum QueueOutcome {
    Continue,
    Send(String),
    Refresh,
    Close(bool),
}

enum QueueReply {
    Refreshed(Result<Agent>, Target),
    Sent(Result<Draft>, Target),
}

fn queue_worker(
    service: &DraftService,
    panel_id: Option<String>,
    send: Option<String>,
) -> Result<UiWorker<QueueReply>> {
    let path = service.store.path.clone();
    let client = service.client.clone();
    let target = service.target.clone();
    UiWorker::spawn("herdr-queue-request", move |cancelled| {
        if cancelled.load(Ordering::Relaxed) {
            return if send.is_some() {
                QueueReply::Sent(Err(interrupted_error()), target)
            } else {
                QueueReply::Refreshed(Err(interrupted_error()), target)
            };
        }
        let service = DraftStore::open(&path)
            .and_then(|store| DraftService::new(store, client, target.clone()));
        match service {
            Ok(mut service) => {
                if let Some(id) = send {
                    let result = service.send(&id);
                    QueueReply::Sent(result, service.target)
                } else {
                    let result = (|| {
                        if let Some(panel_id) = panel_id {
                            service.client.request("pane.report_metadata", serde_json::json!({
                                "pane_id":panel_id,"source":"herdr-drafts", "tokens":{
                                    "drafts_owner":service.target.lock_key(), "drafts_target":service.target.key()
                                },"ttl_ms":10000
                            }))?;
                        }
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(interrupted_error());
                        }
                        service.refresh()
                    })();
                    QueueReply::Refreshed(result, service.target)
                }
            }
            Err(error) if send.is_some() => QueueReply::Sent(Err(error), target),
            Err(error) => QueueReply::Refreshed(Err(error), target),
        }
    })
}

impl QueueApp {
    fn new(service: DraftService, allow_switch: bool) -> Result<Self> {
        let composer = service.store.composer(&service.target)?;
        let drafts = service.store.list_drafts(&service.target, false)?;
        Ok(Self {
            service,
            view: QueueView::new(composer, drafts, allow_switch),
        })
    }
    fn persist(&self) -> Result<()> {
        self.service.store.save_composer(
            &self.service.target,
            &self.view.body(),
            self.view.editing_id.as_deref(),
        )
    }

    fn reply(&mut self, reply: QueueReply) -> Result<()> {
        match reply {
            QueueReply::Refreshed(result, target) => {
                let result = result.and_then(|agent| {
                    self.service.verify_target(&target)?;
                    Ok(agent)
                });
                let status = match result {
                    Ok(agent) if agent.target.socket_path == "/demo" => {
                        "Démo : les envois sont simulés".into()
                    }
                    Ok(agent) => status_label(&agent.status).into(),
                    Err(error) => error.to_string(),
                };
                self.view.dirty |= self.view.status != status;
                self.view.status = status;
            }
            QueueReply::Sent(result, target) => {
                self.view.busy = false;
                self.view.dirty = true;
                self.service.verify_target(&target)?;
                match result {
                    Ok(draft) if draft.state == "sent" => {
                        self.view.notify("Message transmis au terminal.")
                    }
                    Ok(draft) if draft.state == "uncertain" => self
                        .view
                        .notify("Envoi à vérifier dans le terminal. Aucun renvoi automatique."),
                    Ok(draft) => self
                        .view
                        .notify(draft.error.unwrap_or_else(|| "Message conservé.".into())),
                    Err(error) => self.view.notify(error.to_string()),
                }
            }
        }
        self.reload()
    }

    fn reload(&mut self) -> Result<()> {
        let selected = self
            .view
            .drafts
            .get(self.view.selected)
            .map(|draft| draft.id.clone());
        let drafts = self
            .service
            .store
            .list_drafts(&self.service.target, self.view.history)?;
        if self.view.drafts != drafts {
            self.view.drafts = drafts;
            self.view.selected = selected
                .and_then(|id| self.view.drafts.iter().position(|draft| draft.id == id))
                .unwrap_or(
                    self.view
                        .selected
                        .min(self.view.drafts.len().saturating_sub(1)),
                );
            self.view.dirty = true;
        }
        Ok(())
    }

    fn action(&mut self, action: QueueAction) -> Result<QueueOutcome> {
        let confirmed = matches!(action, QueueAction::Confirm(true));
        let action = if let QueueAction::Confirm(accepted) = action {
            self.view.dirty = true;
            let pending = self.view.confirming.take();
            match (accepted, pending) {
                (true, Some(pending)) => pending,
                _ => return Ok(QueueOutcome::Continue),
            }
        } else {
            if self.view.confirming.is_some() {
                return Ok(QueueOutcome::Continue);
            }
            action
        };
        match action {
            QueueAction::Save => {
                self.persist()?;
                self.service.save()?;
                self.view.set_composer("", None);
                self.view.history = false;
                self.view.notify("Message ajouté à la liste.");
                self.reload()?;
            }
            QueueAction::New if !confirmed && !self.view.body().is_empty() => {
                self.view.confirming = Some(QueueAction::New);
                self.view.confirm_yes = false;
                self.view.dirty = true;
            }
            QueueAction::New => {
                self.view.set_composer("", None);
                self.persist()?;
                self.reload()?;
            }
            QueueAction::History => {
                self.view.history = !self.view.history;
                self.view.selected = 0;
                self.view.scroll = 0;
                self.view.dirty = true;
                self.reload()?;
            }
            QueueAction::Send(id) => {
                if self.view.busy || self.view.history {
                    return Ok(QueueOutcome::Continue);
                }
                let draft = self.service.store.get(&id)?;
                if self.view.editing_id.as_deref() == Some(id.as_str())
                    || !matches!(draft.state.as_str(), "draft" | "error")
                {
                    self.view.notify(
                        "Enregistrez la modification ou vérifiez cet envoi avant de continuer.",
                    );
                    return Ok(QueueOutcome::Continue);
                }
                self.persist()?;
                self.view.busy = true;
                self.view.dirty = true;
                self.view.notify("Envoi en cours…");
                return Ok(QueueOutcome::Send(id));
            }
            QueueAction::Edit(id) | QueueAction::Reuse(id) => {
                if self.view.busy {
                    return Ok(QueueOutcome::Continue);
                }
                if !self.view.body().is_empty() {
                    self.view
                        .notify("Ajoutez ou terminez d'abord le message en cours.");
                    return Ok(QueueOutcome::Continue);
                }
                let draft = self.service.store.get(&id)?;
                if !matches!(draft.state.as_str(), "draft" | "error" | "sent") {
                    return Ok(QueueOutcome::Continue);
                }
                let editing_id = if draft.state == "sent" {
                    None
                } else {
                    Some(id)
                };
                self.view.set_composer(&draft.body, editing_id);
                self.persist()?;
                self.reload()?;
            }
            QueueAction::Move(id, direction) if !self.view.busy => {
                self.service.store.move_draft(&id, direction)?;
                self.reload()?;
            }
            action @ (QueueAction::Delete(_) | QueueAction::Resolve(_, _)) if !self.view.busy => {
                if let QueueAction::Delete(id) = &action {
                    if self.view.editing_id.as_deref() == Some(id.as_str()) {
                        self.view.notify(
                            "Enregistrez ou terminez d'abord la modification de ce message.",
                        );
                        return Ok(QueueOutcome::Continue);
                    }
                }
                if !confirmed {
                    self.view.confirming = Some(action);
                    self.view.confirm_yes = false;
                    self.view.dirty = true;
                } else {
                    match action {
                        QueueAction::Delete(id) => self.service.store.delete(&id)?,
                        QueueAction::Resolve(id, delivered) => {
                            self.service.store.resolve(&id, delivered)?
                        }
                        _ => unreachable!(),
                    }
                    self.reload()?;
                }
            }
            QueueAction::Select(index) => {
                self.view.dirty |= self.view.selected != index || self.view.editor_focus;
                self.view.selected = index.min(self.view.drafts.len().saturating_sub(1));
                self.view.editor_focus = false;
                self.view.focus_button = None;
            }
            QueueAction::Refresh if !self.view.busy => return Ok(QueueOutcome::Refresh),
            QueueAction::Quit | QueueAction::Agents => {
                if self.view.busy {
                    self.view.notify(
                        "Attendez la confirmation de l'envoi avant de fermer ou changer d'agent.",
                    );
                } else {
                    self.persist()?;
                    return Ok(QueueOutcome::Close(
                        matches!(action, QueueAction::Agents) && self.view.allow_switch,
                    ));
                }
            }
            _ => {}
        }
        Ok(QueueOutcome::Continue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerAction {
    Choose(usize),
    Refresh,
    Quit,
}

struct PickerView {
    agents: Vec<Agent>,
    visible: Vec<usize>,
    selected: usize,
    search: TextArea<'static>,
    errors: Vec<String>,
    notice: String,
    loading: bool,
    dirty: bool,
    search_focus: bool,
    hits: Vec<(Rect, PickerAction)>,
    search_area: Rect,
    list_area: Rect,
    focus_button: Option<PickerAction>,
}

impl PickerView {
    fn new(notice: &str) -> Self {
        Self {
            agents: Vec::new(),
            visible: Vec::new(),
            selected: 0,
            search: TextArea::default(),
            errors: Vec::new(),
            notice: notice.into(),
            loading: false,
            dirty: true,
            search_focus: true,
            hits: Vec::new(),
            search_area: Rect::default(),
            list_area: Rect::default(),
            focus_button: None,
        }
    }
    fn inventory(&mut self, agents: Vec<Agent>, errors: Vec<String>) {
        let selected = self
            .visible
            .get(self.selected)
            .and_then(|index| self.agents.get(*index))
            .map(|agent| agent.target.key());
        self.dirty |= self.loading || self.agents != agents || self.errors != errors;
        self.loading = false;
        self.agents = agents;
        self.errors = errors;
        self.filter();
        if let Some(key) = selected {
            if let Some(position) = self
                .visible
                .iter()
                .position(|index| self.agents[*index].target.key() == key)
            {
                self.selected = position;
            }
        }
        if matches!(self.focus_button, Some(PickerAction::Choose(_))) {
            self.focus_button = self.choose();
        }
    }

    fn filter(&mut self) {
        let query = self.search.lines().join(" ").trim().to_lowercase();
        let visible = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| {
                [
                    agent.name.as_str(),
                    &agent.target.agent,
                    &agent.workspace_label,
                    &agent.tab_label,
                    &agent.session_label,
                    &agent.target.cwd,
                    &agent.target.pane_id,
                ]
                .join(" ")
                .to_lowercase()
                .contains(&query)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        self.dirty |= self.visible != visible;
        self.visible = visible;
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    fn choose(&self) -> Option<PickerAction> {
        self.visible
            .get(self.selected)
            .copied()
            .map(PickerAction::Choose)
    }

    fn select(&mut self, offset: isize) {
        let selected = self
            .selected
            .saturating_add_signed(offset)
            .min(self.visible.len().saturating_sub(1));
        self.dirty |= selected != self.selected || self.search_focus;
        self.selected = selected;
        self.search_focus = false;
        self.focus_button = None;
    }

    fn tab(&mut self, backwards: bool) {
        let current = if self.search_focus {
            0
        } else {
            match self.focus_button {
                Some(PickerAction::Choose(_)) => 2,
                Some(PickerAction::Refresh) => 3,
                _ => 1,
            }
        };
        let next = if backwards {
            (current + 3) % 4
        } else {
            (current + 1) % 4
        };
        self.search_focus = next == 0;
        self.focus_button = match next {
            2 => self.choose(),
            3 => Some(PickerAction::Refresh),
            _ => None,
        };
        self.dirty = true;
    }

    fn event(&mut self, event: Event) -> Option<PickerAction> {
        match event {
            Event::Resize(_, _) => {
                self.dirty = true;
                None
            }
            Event::Key(key) if key.kind == KeyEventKind::Release => None,
            Event::Key(key) => {
                if matches!(shortcut(key), Some(Shortcut::Quit)) || key.code == KeyCode::Esc {
                    return Some(PickerAction::Quit);
                }
                if matches!(shortcut(key), Some(Shortcut::Refresh)) {
                    return Some(PickerAction::Refresh);
                }
                if key.code == KeyCode::F(2)
                    || (key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('f'))
                    || (key.modifiers.contains(KeyModifiers::SUPER)
                        && key.code == KeyCode::Char('o'))
                {
                    self.dirty |= !self.search_focus;
                    self.search_focus = true;
                    self.focus_button = None;
                    return None;
                }
                match key.code {
                    KeyCode::Enter => self.focus_button.or_else(|| self.choose()),
                    KeyCode::Down if self.search_focus => {
                        self.search_focus = false;
                        self.dirty = true;
                        None
                    }
                    KeyCode::Down => {
                        self.select(1);
                        None
                    }
                    KeyCode::Up => {
                        self.select(-1);
                        None
                    }
                    KeyCode::PageDown => {
                        self.select(4);
                        None
                    }
                    KeyCode::PageUp => {
                        self.select(-4);
                        None
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        self.tab(
                            key.code == KeyCode::BackTab
                                || key.modifiers.contains(KeyModifiers::SHIFT),
                        );
                        None
                    }
                    _ if self.search_focus => {
                        let cursor = self.search.cursor();
                        if self.search.input(key) {
                            self.filter();
                            self.dirty = true;
                        }
                        self.dirty |= self.search.cursor() != cursor;
                        None
                    }
                    _ => None,
                }
            }
            Event::Paste(text) if self.search_focus => {
                if text
                    .chars()
                    .any(|character| terminal_control(character) && character != '\r')
                {
                    self.errors =
                        vec!["Collage refusé : caractère de contrôle du terminal.".into()];
                    self.dirty = true;
                    return None;
                }
                if self.search.insert_str(text.replace(['\r', '\n'], " ")) {
                    self.filter();
                    self.dirty = true;
                }
                None
            }
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if let Some((_, action)) = self
                        .hits
                        .iter()
                        .rev()
                        .find(|(area, _)| area.contains((mouse.column, mouse.row).into()))
                    {
                        return Some(*action);
                    }
                    if self.search_area.contains((mouse.column, mouse.row).into()) {
                        self.search_focus = true;
                        self.focus_button = None;
                        self.dirty = true;
                        place_caret(&mut self.search, self.search_area, mouse.column, mouse.row);
                    }
                } else if self.list_area.contains((mouse.column, mouse.row).into()) {
                    match mouse.kind {
                        MouseEventKind::ScrollDown => self.select(1),
                        MouseEventKind::ScrollUp => self.select(-1),
                        _ => {}
                    }
                }
                None
            }
            _ => None,
        }
    }
}

fn draw_picker(frame: &mut Frame, view: &mut PickerView) {
    let area = frame.area();
    frame.render_widget(Block::default().style(native_style()), area);
    view.hits.clear();
    if area.width < 20 || area.height < 13 {
        frame.render_widget(
            Paragraph::new("Agents Herdr\nAgrandissez le terminal.\nÉchap : fermer")
                .style(native_style())
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(!view.notice.is_empty())),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(view.errors.len().min(2) as u16),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new("Agents Herdr").style(native_style().add_modifier(Modifier::BOLD)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new("Choisissez un agent pour ses messages.").style(native_style()),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(display_text(&view.notice)).style(native_style().fg(Color::Yellow)),
        chunks[2],
    );
    view.search.set_block(
        Block::default()
            .borders(Borders::ALL)
            .title("Rechercher projet, nom ou agent")
            .border_style(native_style().fg(if view.search_focus {
                Color::Cyan
            } else {
                Color::DarkGray
            })),
    );
    view.search.set_style(native_style());
    view.search.set_cursor_line_style(native_style());
    view.search.set_cursor_style(if view.search_focus {
        native_style().add_modifier(Modifier::REVERSED)
    } else {
        native_style()
    });
    view.search_area = chunks[3];
    frame.render_widget(&view.search, chunks[3]);
    let status = if view.loading {
        "Recherche des agents…".into()
    } else if !view.visible.is_empty() {
        format!("{} agent(s) · Cliquez ou Entrée", view.visible.len())
    } else if !view.agents.is_empty() {
        "Aucun agent ne correspond.".into()
    } else if !view.errors.is_empty() {
        "Connexion à Herdr impossible.".into()
    } else {
        "Aucun agent. Lancez-le puis actualisez.".into()
    };
    frame.render_widget(
        Paragraph::new(status).style(native_style().fg(Color::DarkGray)),
        chunks[4],
    );
    frame.render_widget(
        Paragraph::new(display_text(
            &view
                .errors
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        ))
        .style(native_style().fg(Color::Yellow)),
        chunks[5],
    );
    view.list_area = chunks[6];
    let capacity = (chunks[6].height / 3).max(1) as usize;
    let first = view.selected.saturating_sub(capacity.saturating_sub(1));
    for (row, index) in view.visible.iter().skip(first).take(capacity).enumerate() {
        let agent = &view.agents[*index];
        let position = first + row;
        let rect = Rect::new(
            chunks[6].x,
            chunks[6].y + row as u16 * 3,
            chunks[6].width,
            3.min(chunks[6].height.saturating_sub(row as u16 * 3)),
        );
        let project = if agent.workspace_label.is_empty() {
            &agent.target.workspace_id
        } else {
            &agent.workspace_label
        };
        let heading = display_text(&format!(
            "{}{} / {} / {}",
            if position == view.selected {
                "› "
            } else {
                "  "
            },
            project,
            agent.target.agent,
            status_label(&agent.status)
        ));
        let lines = vec![
            Line::styled(
                heading,
                native_style()
                    .fg(if position == view.selected {
                        Color::Cyan
                    } else {
                        Color::Reset
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(display_text(&format!("  {}", agent.name))),
            Line::styled(
                display_text(&format!(
                    "  {} / {}",
                    agent.target.cwd, agent.target.pane_id
                )),
                native_style().fg(Color::DarkGray),
            ),
        ];
        frame.render_widget(Paragraph::new(lines).style(native_style()), rect);
        view.hits.push((rect, PickerAction::Choose(*index)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            ratatui::text::Span::styled(
                "[Ouvrir]",
                native_style().fg(Color::Cyan).add_modifier(
                    if matches!(view.focus_button, Some(PickerAction::Choose(_))) {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                ),
            ),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(
                "[Actualiser]",
                native_style().fg(Color::Cyan).add_modifier(
                    if view.focus_button == Some(PickerAction::Refresh) {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                ),
            ),
        ]))
        .style(native_style()),
        chunks[7],
    );
    if let Some(action) = view.choose() {
        view.hits
            .push((Rect::new(chunks[7].x, chunks[7].y, 8, 1), action));
    }
    if !view.loading {
        view.hits.push((
            Rect::new(chunks[7].x + 10, chunks[7].y, 12, 1),
            PickerAction::Refresh,
        ));
    }
    frame.render_widget(
        Paragraph::new(if cfg!(target_os = "macos") {
            "⌘R Actualiser · ⌘L Fermer · ↑↓ Entrée"
        } else {
            "F5 Actualiser · Échap Fermer · ↑↓ Entrée"
        })
        .style(native_style().fg(Color::DarkGray)),
        chunks[8],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(id: &str, state: &str) -> Draft {
        Draft {
            id: id.into(),
            target_key: "target".into(),
            body: id.into(),
            state: state.into(),
            position: 0,
            error: None,
        }
    }

    #[test]
    fn native_and_fallback_shortcuts_keep_the_existing_actions() {
        for (key, expected) in [
            ('u', Shortcut::Save),
            ('y', Shortcut::SendNext),
            ('l', Shortcut::Quit),
            ('p', Shortcut::New),
            ('o', Shortcut::Agents),
            ('r', Shortcut::Refresh),
        ] {
            assert_eq!(
                shortcut(KeyEvent::new(KeyCode::Char(key), KeyModifiers::SUPER)),
                Some(expected)
            );
        }
        for (key, expected) in [
            ('s', Shortcut::Save),
            ('t', Shortcut::SendNext),
            ('q', Shortcut::Quit),
            ('n', Shortcut::New),
            ('g', Shortcut::Agents),
            ('r', Shortcut::Refresh),
            ('j', Shortcut::Newline),
        ] {
            assert_eq!(
                shortcut(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)),
                Some(expected)
            );
        }
        assert_eq!(
            shortcut(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            Some(Shortcut::SendNext)
        );
        assert_eq!(
            shortcut(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Shortcut::Save)
        );
        assert_eq!(
            shortcut(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(Shortcut::Newline)
        );
        assert_eq!(
            shortcut(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn next_send_uses_visible_order_without_skipping_an_unsafe_first_message() {
        let mut drafts = vec![draft("second", "error"), draft("first", "draft")];
        assert_eq!(
            next_send(&drafts, None, false, false, false),
            Some("second")
        );
        for state in ["sending", "uncertain", "sent"] {
            drafts[0].state = state.into();
            assert_eq!(next_send(&drafts, None, false, false, false), None);
        }
    }

    #[test]
    fn next_send_is_disabled_during_history_confirmation_editing_and_sending() {
        let drafts = vec![draft("first", "draft"), draft("second", "draft")];
        assert_eq!(next_send(&drafts, Some("first"), false, false, false), None);
        assert_eq!(next_send(&drafts, None, true, false, false), None);
        assert_eq!(next_send(&drafts, None, false, true, false), None);
        assert_eq!(next_send(&drafts, None, false, false, true), None);
        assert_eq!(
            next_send(&drafts, Some("second"), false, false, false),
            Some("first")
        );
    }

    #[test]
    fn multiline_paste_inserts_one_unicode_message_and_never_submits() {
        let mut editor = TextArea::default();
        assert!(edit_input(
            &mut editor,
            Event::Paste("café 🎯\r\nseconde\n終".into())
        ));
        assert_eq!(editor.lines().join("\n"), "café 🎯\nseconde\n終");
        assert!(!edit_input(
            &mut editor,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        ));
        assert_eq!(editor.lines().join("\n"), "café 🎯\nseconde\n終");
        assert!(edit_input(
            &mut editor,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
        ));
        assert_eq!(editor.lines().join("\n"), "café 🎯\nseconde\n終\n");
    }

    fn target() -> Target {
        Target {
            socket_path: "/demo".into(),
            pane_id: "w1:p1".into(),
            terminal_id: "terminal".into(),
            workspace_id: "workspace".into(),
            agent: "codex".into(),
            cwd: "/projets/café".into(),
            session: Some("session".into()),
        }
    }

    #[test]
    fn send_next_event_preserves_the_unfinished_unicode_composer() {
        let mut view = QueueView::new(
            Composer {
                body: "en cours 🎯".into(),
                editing_id: None,
            },
            vec![draft("next", "draft"), draft("last", "draft")],
            true,
        );
        let action = view.event(Event::Key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::SUPER,
        )));
        assert_eq!(action, Some(QueueAction::Send("next".into())));
        assert_eq!(view.editor.lines().join("\n"), "en cours 🎯");
        view.confirming = Some(QueueAction::Delete("last".into()));
        assert_eq!(
            view.event(Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE))),
            None
        );
    }

    #[test]
    fn queue_renders_native_colors_and_clickable_send_in_narrow_and_large_terminals() {
        use ratatui::{backend::TestBackend, Terminal};
        for (width, height) in [(36, 24), (120, 40), (12, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut view =
                QueueView::new(Composer::default(), vec![draft("café 🎯", "draft")], true);
            terminal
                .draw(|frame| draw_queue(frame, &mut view, &target()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text = buffer
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("Brouillons"), "{width}×{height}: {text}");
            assert!(buffer
                .content
                .iter()
                .all(|cell| cell.bg == ratatui::style::Color::Reset));
            if width >= 36 {
                assert!(text.contains("Envoyer"));
                assert!(text.contains("café"));
            }
        }
    }

    struct TestQueue {
        app: QueueApp,
        directory: std::path::PathBuf,
    }
    impl TestQueue {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("herdr-ui-{}", crate::models::unique_id()));
            let store = crate::store::DraftStore::open(&directory.join("drafts.sqlite3")).unwrap();
            let service = crate::service::DraftService::demo(store, target()).unwrap();
            Self {
                app: QueueApp::new(service, true).unwrap(),
                directory,
            }
        }
    }
    impl Drop for TestQueue {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn composer_is_durable_and_save_clears_it_without_sending() {
        let mut queue = TestQueue::new();
        queue.app.view.set_composer("premier 🎯\nseconde", None);
        queue.app.persist().unwrap();
        assert_eq!(
            queue
                .app
                .service
                .store
                .composer(&queue.app.service.target)
                .unwrap()
                .body,
            "premier 🎯\nseconde"
        );
        assert_eq!(
            queue.app.action(QueueAction::Save).unwrap(),
            QueueOutcome::Continue
        );
        assert_eq!(queue.app.view.body(), "");
        assert_eq!(queue.app.view.drafts.len(), 1);
        assert_eq!(queue.app.view.drafts[0].state, "draft");
    }

    #[test]
    fn delete_and_new_require_confirmation_without_erasing_other_text() {
        let mut queue = TestQueue::new();
        queue.app.view.set_composer("un message", None);
        queue.app.action(QueueAction::Save).unwrap();
        let id = queue.app.view.drafts[0].id.clone();
        queue.app.view.set_composer("en cours", None);
        queue.app.action(QueueAction::Delete(id.clone())).unwrap();
        assert!(queue.app.view.confirming.is_some());
        queue.app.action(QueueAction::Confirm(false)).unwrap();
        assert_eq!(queue.app.view.drafts.len(), 1);
        queue.app.action(QueueAction::New).unwrap();
        queue.app.action(QueueAction::Confirm(false)).unwrap();
        assert_eq!(queue.app.view.body(), "en cours");
        queue.app.action(QueueAction::Delete(id)).unwrap();
        queue.app.action(QueueAction::Confirm(true)).unwrap();
        assert!(queue.app.view.drafts.is_empty());
        assert_eq!(queue.app.view.body(), "en cours");
    }

    #[test]
    fn edit_reorder_history_and_reuse_keep_their_persistence_contracts() {
        let mut queue = TestQueue::new();
        for body in ["premier", "second"] {
            queue.app.view.set_composer(body, None);
            queue.app.action(QueueAction::Save).unwrap();
        }
        let first = queue.app.view.drafts[0].id.clone();
        let second = queue.app.view.drafts[1].id.clone();
        queue
            .app
            .action(QueueAction::Move(second.clone(), -1))
            .unwrap();
        assert_eq!(queue.app.view.drafts[0].id, second);
        queue.app.action(QueueAction::Edit(first.clone())).unwrap();
        assert_eq!(queue.app.view.body(), "premier");
        queue.app.view.set_composer("modifié", Some(first.clone()));
        queue.app.action(QueueAction::Save).unwrap();
        assert_eq!(queue.app.service.store.get(&first).unwrap().body, "modifié");
        queue.app.service.store.claim(&first).unwrap();
        queue
            .app
            .service
            .store
            .finish(&first, "sent", None)
            .unwrap();
        queue.app.action(QueueAction::History).unwrap();
        assert_eq!(queue.app.view.drafts.len(), 1);
        queue.app.action(QueueAction::Reuse(first)).unwrap();
        assert_eq!(queue.app.view.body(), "modifié");
        assert_eq!(queue.app.view.editing_id, None);
    }

    #[test]
    fn sending_blocks_close_switch_and_a_second_send_while_preserving_composer() {
        let mut queue = TestQueue::new();
        queue.app.view.set_composer("à envoyer", None);
        queue.app.action(QueueAction::Save).unwrap();
        let id = queue.app.view.drafts[0].id.clone();
        queue.app.view.set_composer("encore en cours", None);
        assert_eq!(
            queue.app.action(QueueAction::Send(id.clone())).unwrap(),
            QueueOutcome::Send(id.clone())
        );
        for action in [
            QueueAction::Quit,
            QueueAction::Agents,
            QueueAction::Send(id),
        ] {
            assert_eq!(queue.app.action(action).unwrap(), QueueOutcome::Continue);
        }
        assert_eq!(queue.app.view.body(), "encore en cours");
    }

    fn agent(name: &str, project: &str, pane: &str, kind: &str, socket: &str) -> Agent {
        let mut target = target();
        target.socket_path = socket.into();
        target.pane_id = pane.into();
        target.agent = kind.into();
        Agent {
            target,
            name: name.into(),
            status: "idle".into(),
            workspace_label: project.into(),
            tab_label: "main".into(),
            session_label: String::new(),
        }
    }

    #[test]
    fn picker_searches_name_project_and_agent_type_and_enter_selects_the_filter() {
        let mut picker = PickerView::new("");
        picker.inventory(
            vec![
                agent("Tests", "Forge", "w1:p1", "codex", "/a"),
                agent("Matchs", "OpenGambit", "w2:p1", "claude", "/b"),
            ],
            Vec::new(),
        );
        assert_eq!(picker.visible.len(), 2);
        for query in ["opengambit", "MATCHS", "claude"] {
            picker.search = TextArea::from(vec![query.to_string()]);
            picker.filter();
            assert_eq!(picker.visible, vec![1]);
            assert_eq!(
                picker.event(Event::Key(KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE
                ))),
                Some(PickerAction::Choose(1))
            );
        }
    }

    #[test]
    fn picker_refresh_preserves_complete_identity_and_removes_disappeared_agents() {
        let first = agent("Premier", "Forge", "w1:p1", "codex", "/a");
        let second = agent("Second", "Forge", "w1:p1", "codex", "/b");
        let mut picker = PickerView::new("");
        picker.inventory(vec![first.clone(), second.clone()], Vec::new());
        picker.selected = 1;
        picker.inventory(vec![second, first], Vec::new());
        assert_eq!(
            picker.event(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Some(PickerAction::Choose(0))
        );
        picker.inventory(Vec::new(), vec!["Herdr indisponible".into()]);
        assert_eq!(
            picker.event(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            None
        );
    }

    #[test]
    fn picker_renders_search_inventory_and_errors_without_changing_terminal_background() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let mut picker = PickerView::new("Choisissez un autre agent.");
        picker.inventory(
            vec![agent("Réparer 🎯", "Forge", "p", "codex", "/a")],
            vec!["Une session indisponible".into()],
        );
        terminal
            .draw(|frame| draw_picker(frame, &mut picker))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Forge"));
        assert!(text.contains("Réparer"));
        assert!(text.contains("indisponible"));
        assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
    }

    #[test]
    fn bounded_send_worker_sends_only_the_requested_draft_and_preserves_composer() {
        let mut queue = TestQueue::new();
        for body in ["premier", "second"] {
            queue.app.view.set_composer(body, None);
            queue.app.action(QueueAction::Save).unwrap();
        }
        queue.app.view.set_composer("en cours", None);
        queue.app.persist().unwrap();
        let id = queue.app.view.drafts[1].id.clone();
        let receiver = queue_worker(&queue.app.service, None, Some(id.clone())).unwrap();
        let QueueReply::Sent(result, target) = receiver
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!("expected send reply")
        };
        assert_eq!(result.unwrap().state, "sent");
        assert_eq!(target, queue.app.service.target);
        assert_eq!(queue.app.service.store.get(&id).unwrap().state, "sent");
        assert_eq!(
            queue.app.service.store.composer(&target).unwrap().body,
            "en cours"
        );
        assert_eq!(
            queue
                .app
                .service
                .store
                .list_drafts(&target, false)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn refresh_worker_reports_panel_ownership_with_ttl_before_refreshing_agent() {
        use crate::herdr::test_support::{agent_json, scripted_client};
        use serde_json::json;
        let queue = TestQueue::new();
        let (client, requests) = scripted_client(
            vec![
                Some(json!({"result":{}})),
                Some(json!({"result":{"agent":agent_json()}})),
            ],
            false,
        );
        let target = crate::herdr::test_support::target();
        let service = DraftService::new(
            DraftStore::open(&queue.directory.join("metadata.sqlite3")).unwrap(),
            client,
            target.clone(),
        )
        .unwrap();
        let receiver = queue_worker(&service, Some("draft-panel".into()), None).unwrap();
        let QueueReply::Refreshed(result, _) = receiver
            .receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!("expected refresh reply")
        };
        assert_eq!(result.unwrap().status, "working");
        let mut requests = requests.into_iter();
        let report = requests.next().unwrap().join().unwrap();
        assert_eq!(report["method"], "pane.report_metadata");
        assert_eq!(report["params"]["pane_id"], "draft-panel");
        assert_eq!(report["params"]["ttl_ms"], 10000);
        assert_eq!(
            report["params"]["tokens"]["drafts_owner"],
            target.lock_key()
        );
        assert_eq!(report["params"]["tokens"]["drafts_target"], target.key());
        assert_eq!(
            requests.next().unwrap().join().unwrap()["method"],
            "agent.get"
        );
    }

    #[test]
    fn paste_rejects_escape_and_c1_controls_before_the_editor_can_render_them() {
        let mut editor = TextArea::default();
        for text in ["hello\u{1b}[31m", "hello\u{9b}31m", "hello\u{0}"] {
            assert!(!edit_input(&mut editor, Event::Paste(text.into())));
            assert_eq!(editor.lines().join("\n"), "");
        }
    }

    #[test]
    fn tab_and_shift_tab_focus_actions_and_enter_activates_the_focused_button() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let mut view = QueueView::new(
            Composer {
                body: "message".into(),
                editing_id: None,
            },
            vec![draft("one", "draft")],
            true,
        );
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        view.event(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(
            view.event(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Some(QueueAction::Save)
        );
        view.event(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(
            view.event(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Some(QueueAction::New)
        );
        view.event(Event::Key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
        )));
        assert_eq!(
            view.event(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Some(QueueAction::Save)
        );
    }

    #[test]
    fn editor_mouse_places_the_caret_after_wide_unicode_and_in_a_scrolled_wrapped_line() {
        use crossterm::event::MouseEvent;
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let mut view = QueueView::new(
            Composer {
                body: "ab🎯é終cd".into(),
                editing_id: None,
            },
            Vec::new(),
            false,
        );
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        view.event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: view.editor_area.x + 5,
            row: view.editor_area.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        view.event(Event::Key(KeyEvent::new(
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        )));
        assert_eq!(view.body(), "ab🎯Xé終cd");
        let long = "0123456789".repeat(20);
        view.set_composer(&long, None);
        view.editor.move_cursor(tui_textarea::CursorMove::End);
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        view.event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: view.editor_area.x + 1,
            row: view.editor_area.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(view.editor.cursor(), (0, 102));
        view.event(Event::Key(KeyEvent::new(
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        )));
        assert_eq!(view.body(), format!("{}X{}", &long[..102], &long[102..]));
    }

    #[test]
    fn picker_tab_reaches_open_refresh_and_search_in_both_directions() {
        let mut picker = PickerView::new("");
        picker.inventory(
            vec![agent("Tests", "Forge", "p", "codex", "/a")],
            Vec::new(),
        );
        let tab = || Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let enter = || Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        picker.event(tab());
        picker.event(tab());
        assert_eq!(picker.event(enter()), Some(PickerAction::Choose(0)));
        picker.event(tab());
        assert_eq!(picker.event(enter()), Some(PickerAction::Refresh));
        picker.event(Event::Key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
        )));
        assert_eq!(picker.event(enter()), Some(PickerAction::Choose(0)));
    }

    #[test]
    fn editing_draft_cannot_be_deleted_through_a_keyboard_action() {
        let mut queue = TestQueue::new();
        queue.app.view.set_composer("original", None);
        queue.app.action(QueueAction::Save).unwrap();
        let id = queue.app.view.drafts[0].id.clone();
        queue.app.action(QueueAction::Edit(id.clone())).unwrap();
        queue.app.action(QueueAction::Delete(id.clone())).unwrap();
        assert!(queue.app.view.confirming.is_none());
        assert_eq!(queue.app.service.store.get(&id).unwrap().body, "original");
        assert_eq!(queue.app.view.editing_id, Some(id));
    }

    #[test]
    fn composer_soft_wraps_without_newlines_and_mouse_edits_the_second_visual_line() {
        use crossterm::event::MouseEvent;
        use ratatui::{backend::TestBackend, Terminal};
        let body = "abcdefghij".repeat(5);
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let mut view = QueueView::new(
            Composer {
                body: body.clone(),
                editing_id: None,
            },
            Vec::new(),
            false,
        );
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        let second_row = view.editor_area.y + 2;
        let buffer = terminal.backend().buffer();
        let second_line = (view.editor_area.x + 1..view.editor_area.right() - 1)
            .map(|x| buffer[(x, second_row)].symbol())
            .collect::<String>();
        assert!(second_line.starts_with("efghijabcdefghij"), "{second_line}");
        view.event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: view.editor_area.x + 4,
            row: second_row,
            modifiers: KeyModifiers::NONE,
        }));
        view.event(Event::Key(KeyEvent::new(
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        )));
        assert_eq!(view.body(), format!("{}X{}", &body[..37], &body[37..]));
        assert_eq!(view.editor.lines().len(), 1);
    }

    #[test]
    fn composer_wraps_at_word_boundaries_and_maps_a_wide_character_on_a_wrapped_row() {
        use crossterm::event::MouseEvent;
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let words = "0123456789 abcdefghij 0123456789 abcdefghij";
        let mut view = QueueView::new(
            Composer {
                body: words.into(),
                editing_id: None,
            },
            Vec::new(),
            false,
        );
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        let row = view.editor_area.y + 2;
        let second_line = (1..35)
            .map(|x| terminal.backend().buffer()[(x, row)].symbol())
            .collect::<String>();
        assert!(second_line.starts_with("abcdefghij"), "{second_line}");
        let unicode = format!("{}{}", "é".repeat(20), "🎯".repeat(10));
        view.set_composer(&unicode, None);
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        view.event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(view.editor.cursor(), (0, 28));
        assert_eq!(view.body(), unicode);
    }

    #[test]
    fn narrow_card_mouse_send_and_wheel_navigation_do_not_edit_the_composer() {
        use crossterm::event::MouseEvent;
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let body = (0..20)
            .map(|index| format!("ligne {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut view = QueueView::new(
            Composer {
                body: body.clone(),
                editing_id: None,
            },
            vec![draft("one", "draft"), draft("two", "draft")],
            false,
        );
        terminal
            .draw(|frame| draw_queue(frame, &mut view, &target()))
            .unwrap();
        let (button, _) = view
            .hits
            .iter()
            .find(|(_, action)| action == &QueueAction::Send("one".into()))
            .unwrap()
            .clone();
        assert_eq!(
            view.event(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: button.x,
                row: button.y,
                modifiers: KeyModifiers::NONE
            })),
            Some(QueueAction::Send("one".into()))
        );
        view.event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: view.editor_area.x + 1,
            row: view.editor_area.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(view.editor.cursor().0, 1);
        assert_eq!(view.body(), body);
        view.event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: view.list_area.x,
            row: view.list_area.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(view.selected, 1);
        assert_eq!(view.body(), body);
    }

    #[test]
    fn escape_from_send_button_restores_composer_enter_and_shift_enter_without_sending() {
        use ratatui::{backend::TestBackend, Terminal};
        for newline in [false, true] {
            let mut queue = TestQueue::new();
            queue.app.view.set_composer("ancien message", None);
            queue.app.action(QueueAction::Save).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal
                .draw(|frame| draw_queue(frame, &mut queue.app.view, &target()))
                .unwrap();
            for _ in 0..3 {
                assert_eq!(
                    queue
                        .app
                        .view
                        .event(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))),
                    None
                );
            }
            assert!(matches!(
                queue.app.view.focus_button,
                Some(QueueAction::Send(_))
            ));
            queue
                .app
                .view
                .event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
            for character in "nouveau texte".chars() {
                assert_eq!(
                    queue.app.view.event(Event::Key(KeyEvent::new(
                        KeyCode::Char(character),
                        KeyModifiers::NONE
                    ))),
                    None
                );
            }
            let action = queue.app.view.event(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                if newline {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                },
            )));
            if newline {
                assert_eq!(action, None);
                assert_eq!(queue.app.view.body(), "nouveau texte\n");
                assert_eq!(queue.app.view.drafts.len(), 1);
            } else {
                assert_eq!(action, Some(QueueAction::Save));
                assert_eq!(
                    queue.app.action(action.unwrap()).unwrap(),
                    QueueOutcome::Continue
                );
                assert_eq!(
                    queue
                        .app
                        .view
                        .drafts
                        .iter()
                        .map(|draft| draft.body.as_str())
                        .collect::<Vec<_>>(),
                    vec!["ancien message", "nouveau texte"]
                );
            }
            assert!(queue
                .app
                .service
                .store
                .list_drafts(&target(), true)
                .unwrap()
                .is_empty());
            assert!(!queue.app.view.busy);
        }
    }

    #[test]
    fn dropping_refresh_worker_waits_for_its_identity_adoption_before_releasing_ownership() {
        use crate::herdr::{test_support, HerdrClient};
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        let queue = TestQueue::new();
        let (connection, mut server) = UnixStream::pair().unwrap();
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let server_thread = std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            started_sender.send(()).unwrap();
            release_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            writeln!(server, "{}", serde_json::json!({"id":request["id"],"result":{"agent":test_support::agent_json()}})).unwrap();
        });
        let connection = std::sync::Mutex::new(Some(connection));
        let client = HerdrClient::new("/tmp/herdr-test.sock")
            .with_timeout(Duration::from_millis(500))
            .with_connector(move |_, _| Ok(connection.lock().unwrap().take().unwrap()));
        let mut initial = test_support::target();
        initial.session = None;
        let service = DraftService::new(
            DraftStore::open(&queue.directory.join("ownership.sqlite3")).unwrap(),
            client,
            initial.clone(),
        )
        .unwrap();
        service
            .store
            .save_composer(&initial, "conservé", None)
            .unwrap();
        let worker = queue_worker(&service, None, None).unwrap();
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let release_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            release_sender.send(()).unwrap();
        });
        drop(worker);
        let adopted_when_owner_returns = service.store.resolve_target(&initial).unwrap().session;
        release_thread.join().unwrap();
        server_thread.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while service
            .store
            .resolve_target(&initial)
            .unwrap()
            .session
            .is_none()
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(adopted_when_owner_returns, test_support::target().session);
        assert_eq!(service.store.composer(&initial).unwrap().body, "conservé");
    }

    #[test]
    fn interrupted_terminal_cannot_open_a_new_session_and_returns_a_distinct_error() {
        struct ResetInterruption;
        impl Drop for ResetInterruption {
            fn drop(&mut self) {
                INTERRUPTED.store(false, Ordering::Relaxed);
            }
        }
        let _reset = ResetInterruption;
        interrupt_terminal(libc::SIGTERM);
        let error = TerminalSession::open()
            .err()
            .expect("interruption must prevent reopening");
        assert_eq!(error.code, "interrupted");
        assert!(INTERRUPTED.load(Ordering::Relaxed));
        assert_eq!(read_terminal_event().unwrap_err().code, "interrupted");
        assert!(INTERRUPTED.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelled_inventory_worker_skips_sessions_after_discovery_returns() {
        use std::os::unix::fs::PermissionsExt;
        let queue = TestQueue::new();
        let registry = serde_json::json!({"sessions":[
            {"name":"first","socket_path":"/missing-first.sock","running":true},
            {"name":"second","socket_path":"/missing-second.sock","running":true}
        ]});
        std::fs::write(queue.directory.join("registry.json"), registry.to_string()).unwrap();
        let binary = queue.directory.join("registry");
        std::fs::write(
            &binary,
            b"#!/bin/sh\nsleep 0.05\ncat \"$(dirname \"$0\")/registry.json\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = inventory_worker(
            HashMap::from([(
                "HERDR_BIN_PATH".into(),
                binary.to_string_lossy().into_owned(),
            )]),
            None,
            None,
        )
        .unwrap();
        worker.cancelled.store(true, Ordering::Relaxed);
        let (agents, errors) = worker
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        drop(worker);
        assert!(agents.is_empty());
        assert!(errors.is_empty());
    }
}
