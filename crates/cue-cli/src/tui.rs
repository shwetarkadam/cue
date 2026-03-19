//! Full-screen TUI mode using ratatui + crossterm.
//!
//! Layout:
//! ┌─ Transcript ──────────────────┐┌─ AI Response ─────────────────┐
//! │                               ││                               │
//! │ [MIC] hello world             ││ Here is my response           │
//! │ [SYS] what is a rate limiter? ││ streaming token by token...   │
//! │                               ││                               │
//! └───────────────────────────────┘└───────────────────────────────┘
//! ┌─ Ask (Enter to send, Esc to clear) ──────────────────────────────┐
//! │ > _                                                              │
//! └──────────────────────────────────────────────────────────────────┘

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use cue_core::output::TuiEvent;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
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
    input: String,
    transcript_scroll: u16,
    response_scroll: u16,
    /// Whether mic is actively listening (Ctrl+L toggle)
    listening: bool,
    /// False = auto-scroll response to bottom; set true when user presses PageUp
    response_user_scrolled: bool,
}

impl TuiApp {
    fn new() -> Self {
        Self {
            transcript_lines: Vec::new(),
            responses: Vec::new(),
            current_response: String::new(),
            status: String::from("Press Ctrl+L to start listening | Enter: query AI | q: quit"),
            error: None,
            input: String::new(),
            transcript_scroll: 0,
            response_scroll: 0,
            listening: false,
            response_user_scrolled: false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        let vertical = Layout::vertical([Constraint::Min(8), Constraint::Length(3)])
            .split(frame.area());

        let horizontal = Layout::horizontal([
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ])
        .split(vertical[0]);

        // Transcript panel
        let transcript_lines: Vec<Line> = self
            .transcript_lines
            .iter()
            .map(|(ch, text)| {
                let (label, color) = match ch.as_str() {
                    "system" | "SYS" => ("[SYS]", Color::Cyan),
                    _ => ("[MIC]", Color::Green),
                };
                Line::from(vec![
                    Span::styled(
                        label,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::raw(text.clone()),
                ])
            })
            .collect();

        let (transcript_title, transcript_border_color) = if self.listening {
            (" Transcript  [LISTENING] ", Color::Red)
        } else {
            (" Transcript  [Ctrl+L to listen] ", Color::DarkGray)
        };
        let transcript = Paragraph::new(transcript_lines)
            .block(
                Block::default()
                    .title(transcript_title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(transcript_border_color)),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.transcript_scroll, 0));
        frame.render_widget(transcript, horizontal[0]);

        // Response panel
        let mut response_text = String::new();
        for (i, r) in self.responses.iter().enumerate() {
            if i > 0 {
                response_text.push_str("\n\n─────\n\n");
            }
            response_text.push_str(r);
        }
        if !self.current_response.is_empty() {
            if !response_text.is_empty() {
                response_text.push_str("\n\n─────\n\n");
            }
            response_text.push_str(&self.current_response);
            response_text.push('▋');
        }

        let response = Paragraph::new(response_text)
            .block(
                Block::default()
                    .title(" AI Response ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.response_scroll, 0));
        frame.render_widget(response, horizontal[1]);

        // Input box
        let input_text = format!("> {}_", self.input);
        let input_style = if self.error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let hint = if let Some(ref e) = self.error {
            format!(" ERROR: {} ", e)
        } else {
            format!(" {} ", self.status)
        };
        let input_block = Block::default()
            .title(hint)
            .borders(Borders::ALL)
            .border_style(if self.error.is_some() {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            });
        let input_widget =
            Paragraph::new(Line::from(vec![Span::styled(input_text, input_style)]))
                .block(input_block);
        frame.render_widget(input_widget, vertical[1]);
    }
}

/// Run the TUI event loop. Blocks until the user quits.
pub async fn run_tui(
    mut event_rx: mpsc::Receiver<TuiEvent>,
    query_tx: mpsc::Sender<String>,
    listen_tx: mpsc::Sender<bool>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new();
    let mut should_quit = false;

    while !should_quit {
        terminal.draw(|f| app.render(f))?;

        // Handle terminal input (16ms poll = ~60fps)
        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match (key.code, key.modifiers) {
                    // Always quit on Ctrl+C
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => should_quit = true,

                    // Ctrl+L: toggle listening on/off
                    (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                        app.listening = !app.listening;
                        app.error = None;
                        if app.listening {
                            app.status = String::from("LISTENING... speak now | Ctrl+L to stop | Enter: query AI");
                        } else {
                            app.status = String::from("Stopped. Press Ctrl+L to listen again | Enter: query AI");
                        }
                        let _ = listen_tx.send(app.listening).await;
                    }

                    // Quit on 'q' only when input box is empty
                    (KeyCode::Char('q'), KeyModifiers::NONE) if app.input.is_empty() => {
                        should_quit = true
                    }

                    // Enter: send query (typed text or empty = "use transcript")
                    (KeyCode::Enter, _) => {
                        let query = app.input.clone();
                        app.input.clear();
                        app.error = None;
                        app.status = String::from("Querying AI...");
                        let _ = query_tx.send(query).await;
                    }

                    // Escape: clear input
                    (KeyCode::Esc, _) => {
                        app.input.clear();
                        app.error = None;
                    }

                    // Backspace
                    (KeyCode::Backspace, _) => {
                        app.input.pop();
                    }

                    // Scroll transcript
                    (KeyCode::Up, _) => {
                        app.transcript_scroll = app.transcript_scroll.saturating_sub(1)
                    }
                    (KeyCode::Down, _) => {
                        app.transcript_scroll = app.transcript_scroll.saturating_add(1)
                    }
                    (KeyCode::PageUp, _) => {
                        app.response_user_scrolled = true;
                        app.response_scroll = app.response_scroll.saturating_sub(5)
                    }
                    (KeyCode::PageDown, _) => {
                        app.response_scroll = app.response_scroll.saturating_add(5);
                        // If scrolled all the way back down, resume auto-scroll
                        // (rough heuristic: if user scrolled down many times, re-enable)
                    }

                    // Type into input box
                    (KeyCode::Char(c), _) => app.input.push(c),

                    _ => {}
                }
            }
        }

        // Process events from session
        while let Ok(evt) = event_rx.try_recv() {
            match evt {
                TuiEvent::Transcript { channel, text } => {
                    app.transcript_lines.push((channel, text));
                    // auto-scroll
                    let total = app.transcript_lines.len() as u16;
                    if total > 15 {
                        app.transcript_scroll = total - 15;
                    }
                }
                TuiEvent::ResponseToken(token) => {
                    app.current_response.push_str(&token);
                    // Auto-scroll unless user manually paged up
                    if !app.response_user_scrolled {
                        app.response_scroll = u16::MAX;
                    }
                }
                TuiEvent::ResponseDone => {
                    if !app.current_response.is_empty() {
                        app.responses.push(app.current_response.clone());
                        app.current_response.clear();
                    }
                    app.response_user_scrolled = false;
                    app.response_scroll = u16::MAX;
                    app.status =
                        String::from("Done. Enter: query again | Type to ask manually");
                }
                TuiEvent::Status(s) => {
                    app.status = s;
                    app.error = None;
                }
                TuiEvent::Error(e) => app.error = Some(e),
                TuiEvent::Quit => should_quit = true,
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
