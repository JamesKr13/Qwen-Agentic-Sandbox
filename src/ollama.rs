use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};


#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    pub function: FunctionCall,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    pub name: String,
    /// May arrive as a JSON object OR as a JSON-encoded string — both handled.
    pub arguments: serde_json::Value,
}

impl FunctionCall {
    pub fn args(&self) -> serde_json::Value {
        match &self.arguments {
            serde_json::Value::String(s) => {
                serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
            }
            other => other.clone(),
        }
    }
}


#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    stream: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDef,
}

#[derive(Debug, Serialize, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}


pub fn all_tools() -> Vec<Tool> {
    vec![
        Tool {
            tool_type: "function".into(),
            function: FunctionDef {
                name: "run_command".into(),
                description: "Execute a shell command inside the sandbox and return stdout/stderr/exit-code.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to run." }
                    },
                    "required": ["command"]
                }),
            },
        },
        Tool {
            tool_type: "function".into(),
            function: FunctionDef {
                name: "write_file".into(),
                description: "Create or overwrite a file in the sandbox.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path":    { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            },
        },
        Tool {
            tool_type: "function".into(),
            function: FunctionDef {
                name: "read_file".into(),
                description: "Read a file from the sandbox.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            },
        },
        Tool {
            tool_type: "function".into(),
            function: FunctionDef {
                name: "list_files".into(),
                description: "List a directory. Dirs end with '/'.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Use '.' for sandbox root." }
                    },
                    "required": ["path"]
                }),
            },
        },
        Tool {
            tool_type: "function".into(),
            function: FunctionDef {
                name: "task_complete".into(),
                description: "Call once the task is fully done and verified.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string" }
                    },
                    "required": ["summary"]
                }),
            },
        },
    ]
}


pub struct OllamaReply {
    pub message:  Message,
    pub raw_body: String,
}

pub struct OllamaClient {
    http: Client,
    base: String,
}

impl OllamaClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("HTTP client"),
            base: "http://localhost:11434".into(),
        }
    }

    pub async fn chat(&self, model: &str, messages: Vec<Message>) -> Result<OllamaReply> {
        let req = ChatRequest { model, messages, tools: all_tools(), stream: false };

        let resp = self
            .http
            .post(format!("{}/api/chat", self.base))
            .json(&req)
            .send()
            .await
            .context("connecting to Ollama — is `ollama serve` running?")?;

        let status   = resp.status();
        let raw_body = resp.text().await.context("reading Ollama response")?;

        if !status.is_success() {
            anyhow::bail!("Ollama HTTP {}: {}", status, raw_body);
        }

        let parsed: serde_json::Value =
            serde_json::from_str(&raw_body).context("parsing Ollama JSON")?;

        let message: Message =
            serde_json::from_value(parsed["message"].clone())
                .context("deserialising message")?;

        Ok(OllamaReply { message, raw_body })
    }
}
