use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::AppEvent;
use crate::ollama::{FunctionCall, Message, OllamaClient, ToolCall};
use crate::sandbox::Sandbox;
use std::pin::Pin;
use futures::future::BoxFuture;
use std::collections::HashMap;
use tokio::sync::Mutex;
use tokio::process::Child;

const MODEL: &str = "qwen2.5-coder";

// The system prompt explicitly shows the exact JSON format so the model knows
// how to call tools even when the structured tool_calls path is unavailable.
const SYSTEM: &str = r#"You are an autonomous coding agent in a sandboxed directory.
Work step-by-step. Use tools to accomplish every task. There is not rules except succeding the task.

AVAILABLE TOOLS — call them using the JSON format below:
  run_background: run a shell command in the background, returns pid
  run_command   : run a basic shell command in the sandbox.
  write_file    : create or overwrite a file
  read_file     : read a file
  list_files    : list a directory (use "." for root)
  task_complete : call once the task is finished and verified
  say           : talk to the User 
  plan          : Decompose a complex task into a plan of action
  check_process : check if a pid is still running
  kill_process  : kill a running process by pid


HOW TO CALL A TOOL — output a JSON block like this (and NOTHING else on that turn):
```json
{"name": "run_command", "arguments": {"command": "ls -la"}}
```
```json
{"name": "spawn_process", "arguments": {"commands": "python3 ball.py"}}
```
```json
{"name": "write_file", "arguments": {"path": "hello.py", "content": "print('hi')"}}
```
```json
{"name": "read_file", "arguments": {"path": "hello.py"}}
```
```json
{"name": "list_files", "arguments": {"path": "."}}
```
```json
{"name": "task_complete", "arguments": {"summary": "Created hello.py and verified it runs."}}
```
```json
{"name": "say", "arguments": {"text": "Hello Sir"}}
```
```json
{"name": "plan", "arguments": {"task": "Build a game of pong"}}
```
```json
{"name": "run_background", "arguments": {"command": "python3 server.py"}}
```
```json
{"name": "check_process",  "arguments": {"pid": "12345"}}
```
```json
{"name": "kill_process",   "arguments": {"pid": "12345"}}
```

RULES:
- NEVER access paths outside the sandbox (leading / is stripped, traversal is blocked).
- Always verify your work (run it, test it) before calling task_complete.
- Call only ONE tool per reply. Wait for the result before calling the next.
- For complex tasks call plan first to build a chain of required_steps.
- After you receive a tool result, call the next tool or task_complete."#;

// ── Parsed tool call ──────────────────────────────────────────────────────────

struct ParsedCall {
    name:    String,
    args:    serde_json::Value,
    call_id: Option<String>,
}

// ── Plan types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct SubStep {
    description: String,
}

#[derive(Debug, Clone)]
struct Phase {
    description: String,
    sub_steps:   Vec<SubStep>,
}

#[derive(Debug, Clone)]
struct Plan {
    phases:          Vec<Phase>,
    current_phase:   usize,
    current_substep: usize,
}

impl Plan {
    fn current_phase(&self) -> Option<&Phase> {
        self.phases.get(self.current_phase)
    }

    fn current_substep(&self) -> Option<&SubStep> {
        self.phases
            .get(self.current_phase)?
            .sub_steps
            .get(self.current_substep)
    }

    fn advance(&mut self) {
        let phase = &self.phases[self.current_phase];
        if self.current_substep + 1 < phase.sub_steps.len() {
            self.current_substep += 1;
        } else {
            self.current_phase   += 1;
            self.current_substep  = 0;
        }
    }

    fn is_complete(&self) -> bool {
        self.current_phase >= self.phases.len()
    }

    fn total_substeps(&self) -> usize {
        self.phases.iter().map(|p| p.sub_steps.len()).sum()
    }

    fn completed_substeps(&self) -> usize {
        self.phases[..self.current_phase]
            .iter()
            .map(|p| p.sub_steps.len())
            .sum::<usize>()
            + self.current_substep
    }
}

// ── Agent ─────────────────────────────────────────────────────────────────────

pub struct Agent {
    ollama:  OllamaClient,
    sandbox: Arc<Sandbox>,
    pub tx:  UnboundedSender<AppEvent>,
    processes: Mutex<HashMap<u32, tokio::process::Child>>,
}

impl Agent {
    pub fn new(sandbox: Arc<Sandbox>, tx: UnboundedSender<AppEvent>) -> Self {
        Self { ollama: OllamaClient::new(), sandbox, tx, processes: Mutex::new(HashMap::new()),}
    }

    fn log(&self, msg: impl Into<String>) {
        let _ = self.tx.send(AppEvent::Log(msg.into()));
    }
    fn add_task(&self, msg: impl Into<String>, n: usize) {
        let _ = self.tx.send(AppEvent::AddTask(msg.into(), n));
    }
    fn task_complete(&self) {
        let _ = self.tx.send(AppEvent::TaskComplete);
    }
    fn sep(&self) { self.log("─".repeat(60)); }

    // ── Main loop ─────────────────────────────────────────────────────────────

    pub async fn run_task(&self, task: &str) -> Result<()> {
        self.sep();
        self.log(format!("  🎯  TASK: {}", task));
        self.sep();

        let mut history: Vec<Message> = vec![
            Message {
                role:         "system".into(),
                content:      Some(SYSTEM.into()),
                tool_calls:   None,
                tool_call_id: None,
            },
            Message {
                role:         "user".into(),
                content:      Some(format!("Complete this task:\n{}", task)),
                tool_calls:   None,
                tool_call_id: None,
            },
        ];

        self.run_step(&mut history).await?;

        self.sep();
        let _ = self.tx.send(AppEvent::TaskComplete);
        Ok(())
    }
    fn run_step<'a>(&'a self, history: &'a mut Vec<Message>) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        let mut iters = 0usize;
        const MAX_ITERS: usize = 40;

        loop {
            iters += 1;
            if iters > MAX_ITERS {
                self.log(format!("  ⚠  Reached {} iterations — stopping.", MAX_ITERS));
                break;
            }

            self.log("  🤔  Thinking…");

            let reply = match tokio::time::timeout(
                Duration::from_secs(240),
                self.ollama.chat(MODEL, history.clone()),
            )
            .await
            {
                Ok(r)  => r?,
                Err(_) => return Err(anyhow::anyhow!("Request timed out after 240s")),
            };

            let msg = reply.message.clone();

            if let Some(text) = &msg.content {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    for line in trimmed.lines() {
                        self.log(format!("  💬  {}", line));
                    }
                }
            }

            let calls = self.extract_calls(&msg, &reply.raw_body);
            history.push(msg);

            if calls.is_empty() {
                self.log("  ✅  No tool calls — model finished.");
                break;
            }

            let mut done = false;
            for call in calls {
                let (is_done, result_text) = self.dispatch(&call).await;
                history.push(Message {
                    role:         "tool".into(),
                    content:      Some(result_text),
                    tool_calls:   None,
                    tool_call_id: call.call_id,
                });
                if is_done { done = true; }
            }

            if done { break; }
        }

        Ok(())
    })
}

    async fn run_plan(&self, task: &str) -> Result<()> {
        self.sep();
        self.log(format!("    PLANNING: {}", task));
        self.sep();

        let mut plan = self.plan_task(task).await?;

        while !plan.is_complete() {
            let phase   = plan.current_phase().unwrap();
            let substep = plan.current_substep().unwrap();

            self.log(format!(
                "  ▶  Phase {}/{}: {}",
                plan.current_phase + 1,
                plan.phases.len(),
                phase.description,
            ));
            self.log(format!(
                "     Step {}/{} (overall {}/{}): {}",
                plan.current_substep + 1,
                phase.sub_steps.len(),
                plan.completed_substeps() + 1,
                plan.total_substeps(),
                substep.description,
            ));
            self.sep();

            let mut history = vec![
                Message {
                    role:         "system".into(),
                    content:      Some(SYSTEM.into()),
                    tool_calls:   None,
                    tool_call_id: None,
                },
                Message {
                    role:         "user".into(),
                    content:      Some(format!(
                        "Overall task: {task}\n\n\
                         Current phase ({phase_n}/{phase_total}): {phase}\n\
                         Current sub-step ({step_n}/{step_total}): {step}\n\n\
                         Complete this sub-step fully, then call task_complete.",
                        task        = task,
                        phase_n     = plan.current_phase + 1,
                        phase_total = plan.phases.len(),
                        phase       = phase.description,
                        step_n      = plan.current_substep + 1,
                        step_total  = phase.sub_steps.len(),
                        step        = substep.description,
                    )),
                    tool_calls:   None,
                    tool_call_id: None,
                },
            ];

            self.run_step(&mut history).await?;
            plan.advance();
            self.task_complete();
        }

        self.sep();
        self.log(format!(
            "  ✅  Plan complete — {} phases, {} sub-steps executed.",
            plan.phases.len(),
            plan.total_substeps(),
        ));
        Ok(())
    }

    // ── Planner ───────────────────────────────────────────────────────────────

    async fn plan_phases(&self, task: &str) -> Result<Vec<String>> {
        self.log("    Planning phases…");
        let prompt = format!(
            "Break this task into high-level phases (3–7 phases) THAT YOU ARE ABLE TO DO. \
             Respond ONLY with a JSON array of strings, no preamble, no markdown. \
             Example: [\"Set up project\", \"Implement feature\", \"Test and verify\"]\n\n\
             Task: {task}"
        );
        let reply = self.plan_call(
            &prompt,
            "You are a planning assistant. Output only a JSON array of phase description strings.",
        ).await?;
        parse_string_array(&reply, "phases")
    }

    async fn plan_substeps(&self, task: &str, phase: &str) -> Result<Vec<String>> {
        let prompt = format!(
            "You are planning how to complete one phase of a larger task.\n\n\
             Overall task: {task}\n\
             Current phase: {phase}\n\n\
             Break this phase into concrete, ordered sub-steps (2–6 steps). \
             Each sub-step should be a single, actionable instruction which you are feasibly able todo. \
             Respond ONLY with a JSON array of strings, no preamble, no markdown. \
             Example: [\"Create the file\", \"Write the function\", \"Run the tests\"]"
        );
        let reply = self.plan_call(
            &prompt,
            "You are a planning assistant. Output only a JSON array of sub-step strings.",
        ).await?;
        parse_string_array(&reply, "sub-steps")
    }

    async fn plan_call(&self, prompt: &str, system: &str) -> Result<String> {
        let messages = vec![
            Message {
                role:         "system".into(),
                content:      Some(system.into()),
                tool_calls:   None,
                tool_call_id: None,
            },
            Message {
                role:         "user".into(),
                content:      Some(prompt.into()),
                tool_calls:   None,
                tool_call_id: None,
            },
        ];

        let reply = match tokio::time::timeout(
            Duration::from_secs(60),
            self.ollama.chat(MODEL, messages),
        )
        .await
        {
            Ok(r)  => r?,
            Err(_) => return Err(anyhow::anyhow!("Planning call timed out")),
        };

        Ok(reply.message.content.clone().unwrap_or_default())
    }

    async fn plan_task(&self, task: &str) -> Result<Plan> {
        self.sep();
        self.log("    Building two-level plan…");
        self.sep();

        let phase_descs = self.plan_phases(task).await?;

        let substep_futures: Vec<_> = phase_descs
            .iter()
            .map(|phase| self.plan_substeps(task, phase))
            .collect();

        let substep_results = futures::future::join_all(substep_futures).await;

        let mut phases = Vec::new();
        for (i, (phase_desc, substeps_result)) in
            phase_descs.iter().zip(substep_results).enumerate()
        {
            let mut counter = 1;

            let substeps = substeps_result?;
            self.log(format!("  📦  Phase {}: {}", i + 1, phase_desc));

            let sub_steps: Vec<SubStep> = substeps
                .into_iter()
                .enumerate()
                .map(|(j, desc)| {
                    let idx = counter;
                    counter += 1;

                    self.log(format !("        {}. {}", idx, desc));
                    //self.add_task(format!("        {}. {}", idx, desc), idx);

                    SubStep { description: desc }
                })
                .collect();
            phases.push(Phase { description: phase_desc.clone(), sub_steps });
        }

        self.sep();
        self.log(format!(
            "  ✅  Plan ready: {} phases, {} total sub-steps",
            phases.len(),
            phases.iter().map(|p| p.sub_steps.len()).sum::<usize>()
        ));
        self.sep();

        Ok(Plan { phases, current_phase: 0, current_substep: 0 })
    }

    // ── Tool dispatch ─────────────────────────────────────────────────────────

    fn extract_calls(&self, msg: &Message, raw: &str) -> Vec<ParsedCall> {
        if let Some(tcs) = &msg.tool_calls {
            if !tcs.is_empty() {
                self.log(format!("  [tool-call path: structured × {}]", tcs.len()));
                return tcs
                    .iter()
                    .map(|tc| ParsedCall {
                        name:    tc.function.name.clone(),
                        args:    tc.function.args(),
                        call_id: tc.id.clone(),
                    })
                    .collect();
            }
        }

        let content = msg.content.as_deref().unwrap_or("");
        let parsed = parse_json_tool_calls(content);
        if !parsed.is_empty() {
            self.log(format!("  [tool-call path: content-json × {}]", parsed.len()));
            return parsed;
        }

        let parsed = parse_json_tool_calls(raw);
        if !parsed.is_empty() {
            self.log(format!("  [tool-call path: raw-body × {}]", parsed.len()));
            return parsed;
        }

        vec![]
    }

    async fn dispatch(&self, call: &ParsedCall) -> (bool, String) {
        match self.run_tool(&call.name, &call.args).await {
            Ok(r) if r.starts_with("__DONE__") => {
                (true, r.trim_start_matches("__DONE__").to_string())
            }
            Ok(r)  => (false, r),
            Err(e) => {
                self.log(format!("  ❌  Tool error: {}", e));
                (false, format!("ERROR: {}", e))
            }
        }
    }

    async fn run_tool(&self, name: &str, args: &serde_json::Value) -> Result<String> {
        match name {
            "run_command" => {
                let cmd = args["command"].as_str().unwrap_or("").to_string();
                self.log(format!("  $ {}", cmd));
                let out = self.sandbox.run_command(&cmd).await?;
                if !out.stdout.trim().is_empty() {
                    for line in out.stdout.trim().lines() {
                        self.log(format!("    {}", line));
                    }
                }
                if !out.stderr.trim().is_empty() {
                    for line in out.stderr.trim().lines() {
                        self.log(format!("  ⚠ {}", line));
                    }
                }
                self.log(format!("  [exit {}]", out.exit_code));
                Ok(format!("exit={}\nstdout:\n{}\nstderr:\n{}", out.exit_code, out.stdout, out.stderr))
            }

            "write_file" => {
                let path    = args["path"].as_str().unwrap_or("").to_string();
                let content = args["content"].as_str().unwrap_or("").to_string();
                let lines   = content.lines().count();
                self.log(format!("  📝  write_file  {}  ({} lines)", path, lines));
                self.sandbox.write_file(&path, &content).await?;
                Ok(format!("Written {} ({} lines)", path, lines))
            }

            "read_file" => {
                let path = args["path"].as_str().unwrap_or("").to_string();
                self.log(format!("  📖  read_file   {}", path));
                let content = self.sandbox.read_file(&path).await?;
                Ok(content)
            }

            "list_files" => {
                let path = args["path"].as_str().unwrap_or(".").to_string();
                self.log(format!("  📂  list_files  {}", path));
                let entries = self.sandbox.list_files(&path).await?;
                for e in &entries { self.log(format!("      {}", e)); }
                Ok(entries.join("\n"))
            }

            "task_complete" => {
                let summary = args["summary"].as_str().unwrap_or("").to_string();
                self.log(format!("  ✅  DONE — {}", summary));
                Ok(format!("__DONE__{}", summary))
            }

            "say" => {
                let text = args["text"].as_str().unwrap_or("").to_string();
                self.log(format!("  🗣  {}", text));
                Ok(text)
            }

            // Fix 1: was calling free fn run_plan, missing .await, and Ok() had no value
            "plan" => {
                let task = args["task"].as_str().unwrap_or("").to_string();
                self.run_plan(&task).await?;
                Ok(format!("Plan for '{}' complete.", task))
            }

            "run_background" => {
                let cmd = args["command"].as_str().unwrap_or("").to_string();
                self.log(format!("  $ [bg] {}", cmd));

                let child: tokio::process::Child = self.sandbox.spawn_command(&cmd).await?;
                let pid = child
                    .id()
                    .ok_or_else(|| anyhow::anyhow!("Process exited before PID could be read"))?;

                self.processes.lock().await.insert(pid, child);

                self.log(format!("  🟢  Started background process  pid={}", pid));
                Ok(format!("started pid={}", pid))
            }

            "check_process" => {
                let pid: u32 = args["pid"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("pid must be a number"))?
                    as u32;

                let mut procs = self.processes.lock().await;
                match procs.get_mut(&pid) {
                    None => {
                        self.log(format!("  ❓  pid={} not found in process table", pid));
                        Ok(format!("pid={} unknown", pid))
                    }
                    Some(child) => {
                        let child: &mut tokio::process::Child = child;
                        match child.try_wait()? {
                            None => {
                                self.log(format!("  🟢  pid={} is running", pid));
                                Ok(format!("pid={} running", pid))
                            }
                            Some(status) => {
                                let code: i32 = status.code().unwrap_or(-1);
                                procs.remove(&pid);
                                self.log(format!("  ⚪  pid={} exited  code={}", pid, code));
                                Ok(format!("pid={} exited code={}", pid, code))
                            }
                        }
                    }
                }
            }

            "kill_process" => {
                let pid: u32 = args["pid"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("pid must be a number"))?
                    as u32;

                let mut procs = self.processes.lock().await;
                match procs.get_mut(&pid) {
                    None => {
                        self.log(format!("  ❓  pid={} not found", pid));
                        Ok(format!("pid={} not found", pid))
                    }
                    Some(child) => {
                        let child: &mut tokio::process::Child = child;
                        child.kill().await?;
                        child.wait().await?;
                        procs.remove(&pid);
                        self.log(format!("  🔴  Killed pid={}", pid));
                        Ok(format!("pid={} killed", pid))
                    }
                }
            }

            other => {
                let msg = format!("Unknown tool '{}'", other);
                self.log(format!("  ❓  {}", msg));
                Ok(msg)
            }
        }
    }
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

// Fix 5: parse_string_array was called but never defined
fn parse_string_array(raw: &str, label: &str) -> Result<Vec<String>> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let items: Vec<String> = serde_json::from_str(cleaned)
        .map_err(|e| anyhow::anyhow!("Failed to parse {label}: {e}\nRaw: {cleaned}"))?;

    if items.is_empty() {
        return Err(anyhow::anyhow!("Model returned empty {label}"));
    }

    Ok(items)
}

fn parse_json_tool_calls(text: &str) -> Vec<ParsedCall> {
    let mut calls = Vec::new();
    calls.extend(extract_from_fences(text, "```json", "```"));
    calls.extend(extract_from_fences(text, "```tool", "```"));
    calls.extend(extract_from_tags(text, "<tool_call>", "</tool_call>"));
    if calls.is_empty() {
        calls.extend(extract_bare_json(text));
    }
    calls
}

fn try_parse_call(json_str: &str) -> Option<ParsedCall> {
    let v: serde_json::Value = serde_json::from_str(json_str.trim()).ok()?;
    let name = v["name"].as_str()?.to_string();
    let args = if v["arguments"].is_object() || v["arguments"].is_string() {
        v["arguments"].clone()
    } else if v["parameters"].is_object() {
        v["parameters"].clone()
    } else {
        serde_json::Value::Object(Default::default())
    };
    let args = match &args {
        serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or(args.clone()),
        other => other.clone(),
    };
    Some(ParsedCall { name, args, call_id: None })
}

fn extract_from_fences(text: &str, open: &str, close: &str) -> Vec<ParsedCall> {
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        rest = &rest[start + open.len()..];
        if let Some(end) = rest.find(close) {
            if let Some(c) = try_parse_call(&rest[..end]) { calls.push(c); }
            rest = &rest[end + close.len()..];
        } else { break; }
    }
    calls
}

fn extract_from_tags(text: &str, open: &str, close: &str) -> Vec<ParsedCall> {
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        rest = &rest[start + open.len()..];
        if let Some(end) = rest.find(close) {
            if let Some(c) = try_parse_call(&rest[..end]) { calls.push(c); }
            rest = &rest[end + close.len()..];
        } else { break; }
    }
    calls
}

fn extract_bare_json(text: &str) -> Vec<ParsedCall> {
    let mut calls = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = find_matching_brace(text, i) {
                let slice = &text[i..=end];
                if let Some(c) = try_parse_call(slice) {
                    calls.push(c);
                    i = end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    calls
}

fn find_matching_brace(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if escape             { escape = false; continue; }
        if b == b'\\' && in_string { escape = true; continue; }
        if b == b'"'          { in_string = !in_string; continue; }
        if !in_string {
            if b == b'{'      { depth += 1; }
            else if b == b'}' {
                depth -= 1;
                if depth == 0 { return Some(start + i); }
            }
        }
    }
    None
}