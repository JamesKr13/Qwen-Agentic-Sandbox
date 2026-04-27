use tokio::sync::mpsc::UnboundedSender;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A line to append to the terminal view.
    Log(String),
    AddTask(String, usize),
    /// The currently running task finished.
    TaskComplete,
}

// ── Task queue ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub text: String,
    pub status: TaskStatus,
}

// ── Focus ─────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum Focus {
    Input,
    Tasks,
    Terminal,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    /// All terminal log lines.
    pub logs: Vec<String>,
    /// Task queue.
    pub tasks: Vec<Task>,
    /// Current text in the task input box.
    pub input: String,
    /// Terminal vertical scroll offset (lines from top).
    pub term_scroll: usize,
    /// Which panel has keyboard focus.
    pub focus: Focus,
    /// One-line status bar text.
    pub status: String,
    /// Canonical path of the sandbox root (display only).
    pub sandbox_path: String,
    /// Channel to send events from background tasks.
    pub event_tx: UnboundedSender<AppEvent>,
    /// True while an agent task is running.
    pub agent_running: bool,
    /// Index of task selected in the task list panel.
    pub selected_task: Option<usize>,
}

impl App {
    pub fn new(event_tx: UnboundedSender<AppEvent>, sandbox_path: String) -> Self {
        let mut logs = Vec::new();
        logs.push("╔══════════════════════════════════════════════════════════╗".into());
        logs.push("║          Qwen2.5-Coder  ·  Sandboxed Agent Terminal      ║".into());
        logs.push("╚══════════════════════════════════════════════════════════╝".into());
        logs.push(String::new());
        logs.push(format!("  Sandbox root : {}", sandbox_path));
        logs.push("  Model        : qwen2.5-coder  (via Ollama)".into());
        logs.push(String::new());
        logs.push("  Controls:".into());
        logs.push("    Tab          → cycle focus   (Terminal | Tasks | Input)".into());
        logs.push("    Enter        → submit task".into());
        logs.push("    ↑ / ↓        → scroll terminal  (when Terminal focused)".into());
        logs.push("    Ctrl-C / q   → quit".into());
        logs.push(String::new());
        logs.push("─".repeat(62));
        logs.push(String::new());

        Self {
            logs,
            tasks: Vec::new(),
            input: String::new(),
            term_scroll: 0,
            focus: Focus::Input,
            status: "Idle — add a task in the right panel".into(),
            sandbox_path,
            event_tx,
            agent_running: false,
            selected_task: None,
        }
    }

    // ── Log helpers ───────────────────────────────────────────────────────────

    pub fn push_log(&mut self, line: String) {
        for l in line.lines() {
            self.logs.push(l.to_string());
        }
        // Keep auto-scrolled to bottom when not manually scrolled
        self.scroll_to_bottom();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term_scroll = self.logs.len().saturating_sub(1);
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.term_scroll = self.term_scroll.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        let max = self.logs.len().saturating_sub(1);
        self.term_scroll = (self.term_scroll + n).min(max);
    }

    // ── Task helpers ──────────────────────────────────────────────────────────

    pub fn enqueue(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let t = text.trim().to_string();
        self.push_log(format!("  📥  Queued: {}", t));
        self.tasks.push(Task {
            text: t,
            status: TaskStatus::Pending,
        });
    }

    pub fn insert_task(&mut self, task: String, number: usize) {
        self.tasks.insert(number -1, Task {
            text: task,
            status: TaskStatus::Pending
        });
    } 

    /// Returns the index + text of the first Pending task, if any.
    pub fn next_pending(&self) -> Option<(usize, String)> {
        self.tasks
            .iter()
            .enumerate()
            .find(|(_, t)| t.status == TaskStatus::Pending)
            .map(|(i, t)| (i, t.text.clone()))
    }

    pub fn mark_running(&mut self, idx: usize) {
        if let Some(t) = self.tasks.get_mut(idx) {
            t.status = TaskStatus::Running;
            self.status = format!("Running: {}", t.text);
        }
        self.agent_running = true;
    }

    pub fn mark_done(&mut self) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.status == TaskStatus::Running) {
            t.status = TaskStatus::Done;
        }
        self.agent_running = false;
        self.status = "Idle — add a task in the right panel".into();
    }

    pub fn mark_failed(&mut self) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.status == TaskStatus::Running) {
            t.status = TaskStatus::Failed;
        }
        self.agent_running = false;
        self.status = "Task failed — see log above".into();
    }
}
