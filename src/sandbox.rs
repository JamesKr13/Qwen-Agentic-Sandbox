use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use std::time::Duration;
use tokio::process::Child;

pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    pub async fn new(root: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&root).await?;
        let root = tokio::fs::canonicalize(root).await?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let stripped = rel.trim_start_matches('/');
        let candidate = self.root.join(stripped);

        let safe = if candidate.exists() {
            candidate.canonicalize()?
        } else {
            let mut cur = candidate.clone();
            let mut tail: Vec<std::ffi::OsString> = Vec::new();
            loop {
                if cur.exists() {
                    let mut base = cur.canonicalize()?;
                    for part in tail.into_iter().rev() {
                        base = base.join(part);
                    }
                    break base;
                }
                tail.push(
                    cur.file_name()
                        .ok_or_else(|| anyhow::anyhow!("empty path component"))?
                        .to_owned(),
                );
                cur = cur
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("path has no parent"))?
                    .to_owned();
            }
        };

        if !safe.starts_with(&self.root) {
            bail!("🚫  Path escape blocked: '{}'", rel);
        }
        Ok(safe)
    }

    pub async fn run_command(&self, cmd: &str) -> Result<CommandOutput> {
        let future = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        match tokio::time::timeout(Duration::from_secs(300), future).await {
            Ok(Ok(out)) => Ok(CommandOutput {
                stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                exit_code: out.status.code().unwrap_or(-1),
            }),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok(CommandOutput {
                stdout: String::new(),
                stderr: format!("command timed out after 60s: {}", cmd),
                exit_code: -1,
            }),
        }
    }


    pub async fn spawn_command(&self, cmd: &str) -> Result<Child> {
        let child = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;   // spawn() not output() - returns immediately with Child
        Ok(child)
    }

    pub async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(full, content).await?;
        Ok(())
    }

    pub async fn read_file(&self, path: &str) -> Result<String> {
        let full = self.resolve(path)?;
        Ok(tokio::fs::read_to_string(full).await?)
    }

    pub async fn list_files(&self, path: &str) -> Result<Vec<String>> {
        let full = self.resolve(path)?;

        let mut read_dir = tokio::fs::read_dir(full).await?;
        let mut entries: Vec<String> = Vec::new();

        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(if is_dir {
                format!("{}/", name)
            } else {
                name
            });
        }

        entries.sort();
        Ok(entries)
    }
}

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
