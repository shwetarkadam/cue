//! Full-screen TUI mode using ratatui + crossterm.
//!
//! Layout:
//! ┌─────────────────┬──────────────────────┐
//! │  Transcript     │  AI Response         │
//! │  [SYS] ...      │  (streaming tokens)  │
//! │  [MIC] ...      │                      │
//! └─────────────────┴──────────────────────┘
//! │  status bar                            │
//! └────────────────────────────────────────┘

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use cue_core::output::TuiEvent;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;
use tokio::sync::mpsc;

struct TuiApp {
    /// (channel, text) pairs
    transcript_lines: Vec<(String, String)>,
    /// Completed AI responses
    responses: Vec<String>,
    /// Currently streaming response
    current_response: String,
    status: String,
    error: Option<String>,
    scroll_transcript: u16,
    scroll_response: u16,
}

impl TuiApp {
    fn new() -> Self {
        Self {
            transcript_lines: Vec::new(),
            responses: Vec::new(),
            current_response: String::new(),
            status: String::from("Listening… (q/Ctrl+C to quit | ↑↓ scroll transcript)"),
            error: None,
            scroll_transcript: 0,
            scroll_response: 0,
        }
    }

    fn add_transcript(&mut self, channel: &str, text: &str) {
        self.transcript_lines
            .push((channel.to_string(), text.to_string()));
        // Auto-scroll: keep last ~20 lines visible
        self.scroll_transcript =
            (self.transcript_lines.len() as u16).saturating_sub(20);
    }

    fn add_token(&mut self, token: &str) {
        self.current_response.push_str(token);
        // Auto-scroll response panel
        let line_count = self.current_response.lines().count() as u16;
        self.scroll_response = line_count.saturating_sub(20);
    }

    fn finish_response(&mut self) {
        if !self.current_response.is_empty() {
            self.responses.push(self.current_response.clone());
            self.current_response.clear();
        }
    }

    fn render(&self, frame: &mut Frame) {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(1)])
            .split(frame.area());

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(vertical[0]);

        self.render_transcript(frame, horizontal[0]);
        self.render_response(frame, horizontal[1]);
        self.render_status(frame, vertical[1]);
    }

    fn render_transcript(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self
            .transcript_lines
            .iter()
            .map(|(channel, text)| {
                let (label, color) = match channel.as_str() {
                    "system" | "SYS" => ("[SYS]", Color::Cyan),
                    "mic" | "MIC" => ("[MIC]", Color::Green),
                    _ => ("[???]", Color::White),
                };
                Line::from(vec![
                    Span::styled(
                        label,
                        Style::default()
                            .fg(color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::raw(text.clone()),
                ])
            })
            .collect();

        let block = Block::default()
            .title(" Transcript ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_transcript, 0));

        frame.render_widget(paragraph, area);
    }

    fn render_response(&self, frame: &mut Frame, area: Rect) {
        let mut text = String::new();
        for (i, resp) in self.responses.iter().enumerate() {
            if i > 0 {
                text.push_str("\n\n---\n\n");
            }
            text.push_str(resp);
        }
        if !self.current_response.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n---\n\n");
            }
            text.push_str(&self.current_response);
            text.push('▋'); // blinking cursor indicator
        }

        let block = Block::default()
            .title(" AI Response ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));

        let paragraph = Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_response, 0));

        frame.render_widget(paragraph, area);
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let span = if let Some(ref err) = self.error {
            Span::styled(
                format!(" ERROR: {}", err),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" {}", self.status),
                Style::default().fg(Color::DarkGray),
            )
        };
        frame.render_widget(Paragraph::new(Line::from(span)), area);
    }
}

/// Run the TUI event loop. Blocks until the user quits.
pub async fn run_tui(mut event_rx: mpsc::Receiver<TuiEvent>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new();
    let mut should_quit = false;

    while !should_quit {
        terminal.draw(|f| app.render(f))?;

        // Poll for keyboard input (~60fps)
        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _) => should_quit = true,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => should_quit = true,
                    (KeyCode::Up, _) => {
                        app.scroll_transcript = app.scroll_transcript.saturating_sub(1)
                    }
                    (KeyCode::Down, _) => {
                        app.scroll_transcript = app.scroll_transcript.saturating_add(1)
                    }
                    (KeyCode::PageUp, _) => {
                        app.scroll_response = app.scroll_response.saturating_sub(5)
                    }
                    (KeyCode::PageDown, _) => {
                        app.scroll_response = app.scroll_response.saturating_add(5)
                    }
                    _ => {}
                }
            }
        }

        // Drain pending events from the session
        while let Ok(evt) = event_rx.try_recv() {
            match evt {
                TuiEvent::Transcript { channel, text } => app.add_transcript(&channel, &text),
                TuiEvent::ResponseToken(token) => app.add_token(&token),
                TuiEvent::ResponseDone => app.finish_response(),
                TuiEvent::Status(s) => {
                    app.status = s;
                    app.error = None;
                }
                TuiEvent::Error(e) => app.error = Some(e),
                TuiEvent::Quit => should_quit = true,
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
