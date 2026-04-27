use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph, Wrap,
    },
    Frame,
};

use crate::app::{App, Focus, TaskStatus};

// ── Palette ───────────────────────────────────────────────────────────────────

const C_BG: Color        = Color::Reset;
const C_BORDER: Color    = Color::DarkGray;
const C_ACTIVE: Color    = Color::Cyan;
const C_TITLE: Color     = Color::White;
const C_DIM: Color       = Color::DarkGray;

const C_CMD: Color       = Color::Cyan;
const C_FILE_OP: Color   = Color::Blue;
const C_THINK: Color     = Color::DarkGray;
const C_CHAT: Color      = Color::Magenta;
const C_OK: Color        = Color::Green;
const C_WARN: Color      = Color::Yellow;
const C_ERR: Color       = Color::Red;
const C_TASK: Color      = Color::Yellow;
const C_SEP: Color       = Color::DarkGray;

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // Outer vertical: [main area] / [status bar]
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(size);

    // Main area: [terminal 65%] / [sidebar 35%]
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(outer[0]);

    // Sidebar: [task list] / [input box]
    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(7)])
        .split(main[1]);

    draw_terminal(frame, app, main[0]);
    draw_task_list(frame, app, sidebar[0]);
    draw_input(frame, app, sidebar[1]);
    draw_status(frame, app, outer[1]);
}

// ── Terminal pane ─────────────────────────────────────────────────────────────

fn classify(line: &str) -> Style {
    if line.starts_with("  $ ")          { Style::default().fg(C_CMD).add_modifier(Modifier::BOLD) }
    else if line.starts_with("  📝")
         || line.starts_with("  📖")
         || line.starts_with("  📂")     { Style::default().fg(C_FILE_OP) }
    else if line.starts_with("  🤔")     { Style::default().fg(C_THINK) }
    else if line.starts_with("  💬")     { Style::default().fg(C_CHAT) }
    else if line.starts_with("  ✅")     { Style::default().fg(C_OK).add_modifier(Modifier::BOLD) }
    else if line.starts_with("  ⚠")      { Style::default().fg(C_WARN) }
    else if line.starts_with("  ❌")     { Style::default().fg(C_ERR).add_modifier(Modifier::BOLD) }
    else if line.starts_with("  🎯")     { Style::default().fg(C_TASK).add_modifier(Modifier::BOLD) }
    else if line.starts_with("  📥")     { Style::default().fg(C_ACTIVE) }
    else if line.starts_with("  [exit")  { Style::default().fg(C_DIM) }
    else if line.starts_with("─") || line.starts_with("╔") || line.starts_with("║") || line.starts_with("╚") {
        Style::default().fg(C_SEP)
    }
    else                                  { Style::default().fg(Color::White) }
}

fn draw_terminal(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Terminal;
    let border_style = if focused {
        Style::default().fg(C_ACTIVE)
    } else {
        Style::default().fg(C_BORDER)
    };

    // Visible line count inside the block (subtract 2 for borders)
    let inner_h = area.height.saturating_sub(2) as usize;

    // Compute the window of lines to display
    let total = app.logs.len();
    let start = if total > inner_h {
        // Clamp scroll so we never show fewer lines than available
        app.term_scroll.saturating_sub(inner_h.saturating_sub(1)).min(total - inner_h)
    } else {
        0
    };
    let end = (start + inner_h).min(total);

    let visible: Vec<Line> = app.logs[start..end]
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), classify(l))))
        .collect();

    let scroll_hint = if total > inner_h {
        format!(
            " Terminal  [{}/{}] ",
            (app.term_scroll + 1).min(total),
            total
        )
    } else {
        " Terminal ".into()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(
            scroll_hint,
            Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD),
        ));

    let para = Paragraph::new(visible)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(para, area);

    // Scrollbar hint in bottom-right corner when focused
    if focused && total > inner_h {
        let hint = Span::styled(" ↑↓ scroll ", Style::default().fg(C_DIM));
        let x = area.x + area.width.saturating_sub(12);
        let y = area.y + area.height.saturating_sub(1);
        if x < area.x + area.width && y < area.y + area.height {
            frame.render_widget(
                Paragraph::new(hint),
                Rect { x, y, width: 12, height: 1 },
            );
        }
    }
}

// ── Task list ─────────────────────────────────────────────────────────────────

fn draw_task_list(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Tasks;
    let border_style = if focused {
        Style::default().fg(C_ACTIVE)
    } else {
        Style::default().fg(C_BORDER)
    };

    let items: Vec<ListItem> = if app.tasks.is_empty() {
        vec![ListItem::new(Span::styled(
            "  (no tasks yet)",
            Style::default().fg(C_DIM).add_modifier(Modifier::ITALIC),
        ))]
    } else {
        app.tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (icon, style) = match t.status {
                    TaskStatus::Pending => (
                        "○",
                        Style::default().fg(Color::White),
                    ),
                    TaskStatus::Running => (
                        "▶",
                        Style::default()
                            .fg(C_WARN)
                            .add_modifier(Modifier::BOLD),
                    ),
                    TaskStatus::Done => (
                        "✓",
                        Style::default().fg(C_OK),
                    ),
                    TaskStatus::Failed => (
                        "✗",
                        Style::default().fg(C_ERR),
                    ),
                };
                let num = format!("{:>2}. {} {}", i + 1, icon, t.text);
                let selected = app.selected_task == Some(i);
                let bg = if selected { Color::DarkGray } else { C_BG };
                ListItem::new(Span::styled(num, style.bg(bg)))
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(Span::styled(
                    " 📋 Task Queue ",
                    Style::default().fg(C_TITLE).add_modifier(Modifier::BOLD),
                )),
        );

    frame.render_widget(list, area);
}

// ── Task input ────────────────────────────────────────────────────────────────

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Input;
    let border_style = if focused {
        Style::default().fg(C_ACTIVE)
    } else {
        Style::default().fg(C_BORDER)
    };

    // Show blinking cursor character when focused
    let display = if focused {
        format!("{}_", app.input)
    } else {
        app.input.clone()
    };

    let placeholder = if app.input.is_empty() && !focused {
        Span::styled(
            "  type a task, then press Enter…",
            Style::default()
                .fg(C_DIM)
                .add_modifier(Modifier::ITALIC),
        )
    } else {
        Span::styled(display, Style::default().fg(Color::White))
    };

    let title = if focused {
        Span::styled(
            " ✏  New Task  (Enter → submit) ",
            Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " ✏  New Task ",
            Style::default().fg(C_TITLE),
        )
    };

    let para = Paragraph::new(placeholder)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(title),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(para, area);
}

// ── Status bar ────────────────────────────────────────────────────────────────

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (indicator, color) = if app.agent_running {
        ("⚙ ", C_WARN)
    } else {
        ("● ", C_OK)
    };

    let text = format!(
        " {}{}   │  Sandbox: {}   │  Tab: cycle focus  │  Ctrl-C / q: quit",
        indicator, app.status, app.sandbox_path
    );

    let para = Paragraph::new(Span::styled(text, Style::default().fg(color))).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(C_BORDER)),
    );

    frame.render_widget(para, area);
}
