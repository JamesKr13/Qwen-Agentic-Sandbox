# Qwen2.5-Coder Sandbox Terminal

A Ratatui-powered terminal UI that lets you watch **Qwen2.5-Coder** (via Ollama)
autonomously run commands, write code, and operate entirely within a sandboxed
workspace, while you queue up tasks from a side panel in real time. Qwen2.5-coder works sufficently but prefer certain commands over others stopping progress. 

```
╔══════════════════════════════════════╦═══════════════════════╗
║  Terminal Output                     ║  📋 Task Queue        ║
║                                      ║  1. ○ write FizzBuzz  ║
║  ─────────────────────────────────   ║  2. ▶ add unit tests  ║
║  🎯  TASK: add unit tests            ║  3. ○ benchmark it    ║
║  🤔  Thinking…                       ╠═══════════════════════╣
║  $ cargo test                        ║  ✏ New Task           ║
║    running 4 tests …                 ║  > benchmark it_      ║
╠══════════════════════════════════════╩═══════════════════════╣
║  ⚙ Running: add unit tests  │  Sandbox: /…/sandbox_workspace ║
╚═══════════════════════════════════════════════════════════════╝
```

---

## Requirements

| Dependency | Install |
|---|---|
| **Rust** ≥ 1.75 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **Ollama** | https://ollama.com |
| **Qwen2.5-Coder model** | `ollama pull qwen2.5-coder` |

---

## Quick Start

```bash
# 1. Clone / copy this project
cd qwen-sandbox

# 2. Pull the model (one-time)
ollama pull qwen2.5-coder

# 3. Build and run
cargo run --release
```

The sandbox workspace is created at `./sandbox_workspace/` relative to where
you launch the binary.  The agent cannot read or write outside that directory.

---

## Controls

| Key | Action |
|---|---|
| **Tab** | Cycle focus: Input → Tasks → Terminal → Input |
| **Enter** | Submit task (when Input is focused) |
| **↑ / ↓** | Scroll terminal output (when Terminal is focused) |
| **PgUp / PgDn** | Scroll terminal faster |
| **Home / End** | Jump to top / bottom of log |
| **Ctrl-C** or **q** | Quit |

---

## How it works

1. You type a natural-language task in the **New Task** input and press Enter.
2. Tasks are queued.  As soon as the previous task finishes the agent picks up
   the next one automatically.
3. The agent sends the task to **Ollama** (`qwen2.5-coder`) with a set of tools:

   | Tool | What it does |
   |---|---|
   | `run_command` | Run a shell command in the sandbox |
   | `write_file`  | Create/overwrite a file |
   | `read_file`   | Read a file |
   | `list_files`  | List a directory |
   | `task_complete` | Signal the task is done |
   | `say` | Communicate somthing |
   | `run_background` | Signal the task is done |
   | `kill_process` | Terminates a process by PID |
   | `check_process` | Confirms a process is running  |
   | `plan` | Plans out a task |

5. The agent loops — calling tools, feeding results back — until it calls
   `task_complete` or stops producing tool calls.
6. All output is streamed live to the terminal panel on the left.

---
## Planning out complex task

The system queries the AI to construct a details phases for a complex task for which
it could achieve. For each phases further construct a detailed plan of action of complete
Planning how a phases maybe be acheived. From which the AI follows the plan as if the user
Assigned them as tasks. AI has a option of futher tool calls or completing said task.
---

## Sandbox safety

* Every file path the agent provides is validated against the sandbox root
  before any operation.
* Leading `/` characters are stripped so absolute-looking paths are treated
  as relative.
* Path traversal attempts (`../../etc/passwd`) are detected and blocked.
* Commands run with `sh -c` inside the sandbox directory.  There is no
  kernel-level isolation (no `chroot`), so treat this as a convenience
  guardrail rather than a security boundary.  Run inside a VM or container
  if stronger isolation is needed.

---

## Configuration

Edit `src/agent.rs` to:
* Change `MODEL` constant to use a different Ollama model.
* Modify the `SYSTEM` prompt to adjust agent behaviour.

Edit `src/ollama.rs → all_tools()` to add more tools.

Edit `src/main.rs` to change the sandbox path (default `./sandbox_workspace`).
