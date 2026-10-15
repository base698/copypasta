//! Terminal view for a herdr tab: search box, clip list, and the selected
//! clip's content and lineage.

use anyhow::Result;
use copypasta_core::{context::truncate, graph, ClipKind, ClipRow, Store};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::time::{Duration, Instant};

struct App {
    store: Store,
    query: String,
    rows: Vec<ClipRow>,
    list: ListState,
    detail: String,
    detail_for: Option<i64>,
    status: String,
    scroll: u16,
}

fn place_color(kind: &str) -> Color {
    match kind {
        "herdr" => Color::Magenta,
        "slack" => Color::LightMagenta,
        "web" => Color::Blue,
        "system" => Color::Yellow,
        _ => Color::Gray,
    }
}

impl App {
    fn reload(&mut self) {
        let keep = self.selected().map(|r| r.clip.id);
        self.rows = self.store.search(&self.query, 300).unwrap_or_default();
        let idx = keep.and_then(|id| self.rows.iter().position(|r| r.clip.id == id)).unwrap_or(0);
        self.list.select(if self.rows.is_empty() { None } else { Some(idx) });
        self.refresh_detail(false);
    }

    fn selected(&self) -> Option<&ClipRow> {
        self.list.selected().and_then(|i| self.rows.get(i))
    }

    fn refresh_detail(&mut self, force: bool) {
        let id = self.selected().map(|r| r.clip.id);
        if id == self.detail_for && !force {
            return;
        }
        self.detail_for = id;
        self.scroll = 0;
        self.detail = match self.selected() {
            None => String::new(),
            Some(r) => {
                let body = match r.clip.kind {
                    ClipKind::Image => format!(
                        "[image {}×{}]{}",
                        r.clip.width.unwrap_or(0),
                        r.clip.height.unwrap_or(0),
                        r.clip.text.as_deref().map(|u| format!("  {u}")).unwrap_or_default()
                    ),
                    _ if r.clip.secret => r.clip.preview.clone(),
                    _ => truncate(r.clip.text.as_deref().unwrap_or(&r.clip.preview), 1200),
                };
                let lineage = graph::build(&self.store, r.clip.id).map(|g| graph::render_text(&g)).unwrap_or_default();
                format!(
                    "{body}\n\n── copied {}× · pasted {}× · first {} ──\n\n{lineage}",
                    r.clip.copies,
                    r.clip.pastes,
                    graph::fmt_ts(r.clip.first_seen)
                )
            }
        };
    }

    fn move_sel(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let i = self.list.selected().unwrap_or(0) as isize + delta;
        self.list.select(Some(i.clamp(0, self.rows.len() as isize - 1) as usize));
        self.refresh_detail(false);
    }

    fn copy_selected(&mut self) {
        let Some(r) = self.selected().cloned() else { return };
        if r.clip.secret {
            self.status = "secret clips aren't stored, nothing to copy".into();
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use copypasta_core::pasteboard;
            let ok = match r.clip.kind {
                ClipKind::Image => self
                    .store
                    .read_blob(&r.clip)
                    .map(|png| pasteboard::write_png(&png, r.clip.text.as_deref()))
                    .unwrap_or(false),
                _ => pasteboard::write_text(r.clip.text.as_deref().unwrap_or("")),
            };
            self.status = if ok { format!("copied #{} to clipboard", r.clip.id) } else { "clipboard write failed".into() };
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.status = "copy from the TUI is macOS-only for now".into();
        }
    }
}

pub fn run() -> Result<()> {
    let mut app = App {
        store: Store::open()?,
        query: String::new(),
        rows: vec![],
        list: ListState::default(),
        detail: String::new(),
        detail_for: None,
        status: String::new(),
        scroll: 0,
    };
    app.reload();
    let mut terminal = ratatui::init();
    let mut last_reload = Instant::now();
    let res = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &mut app)) {
            break Err(e.into());
        }
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                match k.code {
                    KeyCode::Char('c') if ctrl => break Ok(()),
                    KeyCode::Esc if app.query.is_empty() => break Ok(()),
                    KeyCode::Esc => {
                        app.query.clear();
                        app.reload();
                    }
                    KeyCode::Char('u') if ctrl => {
                        app.query.clear();
                        app.reload();
                    }
                    KeyCode::Char('n') if ctrl => app.move_sel(1),
                    KeyCode::Char('p') if ctrl => app.move_sel(-1),
                    KeyCode::Char('d') if ctrl => app.scroll = app.scroll.saturating_add(10),
                    KeyCode::Char('e') if ctrl => app.scroll = app.scroll.saturating_sub(10),
                    KeyCode::Down => app.move_sel(1),
                    KeyCode::Up => app.move_sel(-1),
                    KeyCode::PageDown => app.move_sel(15),
                    KeyCode::PageUp => app.move_sel(-15),
                    KeyCode::Enter => app.copy_selected(),
                    KeyCode::Backspace => {
                        app.query.pop();
                        app.reload();
                    }
                    KeyCode::Char(c) if !ctrl => {
                        app.query.push(c);
                        app.reload();
                    }
                    _ => {}
                }
            }
        } else if last_reload.elapsed() > Duration::from_secs(2) {
            last_reload = Instant::now();
            app.reload();
            app.refresh_detail(true);
        }
    };
    ratatui::restore();
    res
}

fn draw(f: &mut Frame, app: &mut App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(1)])
        .split(f.area());
    let search = Paragraph::new(Line::from(vec![
        Span::styled("⌕ ", Style::default().fg(Color::Yellow)),
        Span::raw(app.query.clone()),
        Span::styled("▏", Style::default().fg(Color::Yellow)),
    ]))
    .block(Block::default().borders(Borders::ALL).title(format!(" copypasta · {} clips ", app.rows.len())));
    f.render_widget(search, outer[0]);

    let wide = outer[1].width >= 100;
    let body = Layout::default()
        .direction(if wide { Direction::Horizontal } else { Direction::Vertical })
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(outer[1]);

    let width = body[0].width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|r| {
            let badge = match r.clip.kind {
                ClipKind::Image => "▣",
                ClipKind::Url => "↗",
                ClipKind::File => "▤",
                ClipKind::Text => "¶",
            };
            let mut places = vec![];
            if let Some(p) = r.sources.first() {
                places.push(Span::styled(format!("{} ", truncate(&p.label, 28)), Style::default().fg(place_color(&p.kind))));
            }
            for p in r.destinations.iter().take(2) {
                places.push(Span::styled(format!("→ {} ", truncate(&p.label, 22)), Style::default().fg(place_color(&p.kind)).add_modifier(Modifier::DIM)));
            }
            places.push(Span::styled(graph::fmt_ts(r.clip.last_seen)[11..].to_string(), Style::default().fg(Color::DarkGray)));
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(format!("{badge} "), Style::default().fg(Color::Yellow)),
                    Span::raw(truncate(&r.clip.preview, width.saturating_sub(2))),
                ]),
                Line::from(places),
            ])
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" history "))
        .highlight_style(Style::default().bg(Color::Rgb(60, 52, 30)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, body[0], &mut app.list);

    let detail = Paragraph::new(app.detail.clone())
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0))
        .block(Block::default().borders(Borders::ALL).title(" clip · copy/paste path "));
    f.render_widget(detail, body[1]);

    let help = if app.status.is_empty() {
        "type to search · ↑↓ select · ⏎ copy · ^D/^E scroll lineage · esc clear/quit".to_string()
    } else {
        app.status.clone()
    };
    f.render_widget(Paragraph::new(help).dark_gray(), outer[2]);
}
