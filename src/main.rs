use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;

mod agent;
mod app;
mod ollama;
mod sandbox;
mod ui;

use agent::Agent;
use app::{App, AppEvent, Focus};
use sandbox::Sandbox;


#[tokio::main]
async fn main() -> Result<()> {
    let sandbox_dir = PathBuf::from("./sandbox_workspace");
    let sandbox = Arc::new(Sandbox::new(sandbox_dir.clone()).await?);
    let sandbox_display = sandbox_dir
        .canonicalize()
        .unwrap_or(sandbox_dir)
        .to_string_lossy()
        .to_string();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(event_tx.clone(), sandbox_display);

    let result = run_loop(&mut terminal, &mut app, &mut event_rx, event_tx, sandbox).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}


async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    event_rx: &mut mpsc::UnboundedReceiver<AppEvent>,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    sandbox: Arc<Sandbox>,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Global quit
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    break;
                }
                if key.code == KeyCode::Char('q')
                    && app.focus != Focus::Input
                {
                    break;
                }

                match key.code {
                    KeyCode::Tab => {
                        app.focus = match app.focus {
                            Focus::Input    => Focus::Tasks,
                            Focus::Tasks    => Focus::Terminal,
                            Focus::Terminal => Focus::Input,
                        };
                    }

                    KeyCode::Char(c) if app.focus == Focus::Input => {
                        app.input.push(c);
                    }
                    KeyCode::Backspace if app.focus == Focus::Input => {
                        app.input.pop();
                    }
                    KeyCode::Enter if app.focus == Focus::Input => {
                        let text = app.input.clone();
                        app.input.clear();
                        app.enqueue(text);
                    }

                    KeyCode::Up if app.focus == Focus::Terminal => {
                        app.scroll_up(3);
                    }
                    KeyCode::Down if app.focus == Focus::Terminal => {
                        app.scroll_down(3);
                    }
                    KeyCode::PageUp if app.focus == Focus::Terminal => {
                        app.scroll_up(20);
                    }
                    KeyCode::PageDown if app.focus == Focus::Terminal => {
                        app.scroll_down(20);
                    }
                    KeyCode::Home if app.focus == Focus::Terminal => {
                        app.term_scroll = 0;
                    }
                    KeyCode::End if app.focus == Focus::Terminal => {
                        app.scroll_to_bottom();
                    }

                    _ => {}
                }
            }
        }

        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                AppEvent::Log(line)   => app.push_log(line),
                AppEvent::AddTask(task, number) => app.insert_task(task, number),
                AppEvent::TaskComplete => app.mark_done(),
            }
        }

        if !app.agent_running {
            if let Some((idx, task_text)) = app.next_pending() {
                app.mark_running(idx);
                let sb  = Arc::clone(&sandbox);
                let tx  = event_tx.clone();
                let txt = task_text.clone();

                tokio::spawn(async move {
                    let agent = Agent::new(sb, tx.clone());
                    if let Err(e) = agent.run_task(&txt).await {
                        let _ = tx.send(AppEvent::Log(format!("  ❌  Agent error: {}", e)));
                        let _ = tx.send(AppEvent::TaskComplete);
                    }
                });
            }
        }
    }

    Ok(())
}
