use crate::core::error::AgentError;
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info};

/// Default per-send() timeout in seconds (10 min for discovery with many tool turns).
const DEFAULT_SEND_TIMEOUT_SECS: u64 = 600;

/// A multi-turn Claude CLI session backed by the bootstrap file pattern.
///
/// Each `send()` call:
/// 1. Writes the full prompt to `<temp>/dig2crawl_<pid>/prompt.md`
/// 2. Writes the expected output path to `<temp>/dig2crawl_<pid>/response.json`
/// 3. Runs `claude --dangerously-skip-permissions -p "<short bootstrap>"` where
///    the bootstrap instructs Claude to read `prompt.md` and write JSON to `response.json`
/// 4. Reads `response.json` from disk after the process exits
///
/// This avoids Windows cmd.exe argument-length limits (8191 chars) by keeping
/// the `-p` argument under 300 bytes and storing the real prompt in a file.
///
/// On subsequent calls `--resume <session_id>` is passed so Claude retains
/// full conversational context.
///
/// # Example
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use std::path::Path;
/// let mut session = crawl_agent::session::AgentSession::start().await?;
/// let raw = session.send_with_files(
///     Path::new("/tmp/prompt.md"),
///     Path::new("/tmp/response.json"),
/// ).await?;
/// # Ok(())
/// # }
/// ```
pub struct AgentSession {
    claude_bin: PathBuf,
    session_id: Option<String>,
    timeout_secs: u64,
}

impl AgentSession {
    /// Locate the claude CLI binary and create a session handle.
    ///
    /// No process is spawned here. The binary is located by trying common
    /// installation paths. Returns an error if none are found.
    pub async fn start() -> Result<Self, AgentError> {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();

        let mut candidates = vec![
            PathBuf::from("claude"),
            PathBuf::from("claude.cmd"),
        ];
        if !home.is_empty() {
            let h = PathBuf::from(&home);
            candidates.push(h.join("AppData/Roaming/npm/claude.cmd"));
            candidates.push(h.join("AppData/Roaming/npm/claude"));
            candidates.push(h.join(".local/bin/claude"));
        }
        candidates.push(PathBuf::from("/usr/local/bin/claude"));

        for bin in candidates {
            if Self::probe_bin(&bin).await {
                info!(binary = %bin.display(), "AgentSession ready");
                return Ok(Self {
                    claude_bin: bin,
                    session_id: None,
                    timeout_secs: DEFAULT_SEND_TIMEOUT_SECS,
                });
            }
        }

        Err(AgentError::Spawn(
            "claude CLI not found. Install with: npm install -g @anthropic-ai/claude-code".into(),
        ))
    }

    /// Same as `start` but with an explicit binary path, skipping discovery.
    pub async fn start_with_bin(claude_bin: PathBuf) -> Result<Self, AgentError> {
        if !Self::probe_bin(&claude_bin).await {
            return Err(AgentError::Spawn(format!(
                "claude binary not executable: {}",
                claude_bin.display()
            )));
        }
        info!(binary = %claude_bin.display(), "AgentSession ready");
        Ok(Self {
            claude_bin,
            session_id: None,
            timeout_secs: DEFAULT_SEND_TIMEOUT_SECS,
        })
    }

    /// Override the per-send() timeout (default: 600 s).
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Send a prompt using the bootstrap file pattern.
    ///
    /// - `prompt_path`: path to the prompt file that Claude will read with its Read tool
    /// - `response_path`: path where Claude must write its JSON response
    ///
    /// The caller is responsible for writing `prompt_path` before calling this method.
    /// After this returns `Ok(())`, the caller should read `response_path` from disk.
    ///
    /// On the first call a fresh conversation is started. On subsequent calls
    /// `--resume <session_id>` is added so Claude retains full prior context.
    pub async fn send_with_files(
        &mut self,
        prompt_path: &std::path::Path,
        response_path: &std::path::Path,
    ) -> Result<(), AgentError> {
        debug!(
            prompt = %prompt_path.display(),
            response = %response_path.display(),
            "AgentSession::send_with_files",
        );

        let secs = self.timeout_secs;
        timeout(
            std::time::Duration::from_secs(secs),
            self.do_send(prompt_path, response_path),
        )
        .await
        .map_err(|_| AgentError::Timeout { secs })?
    }

    async fn do_send(
        &mut self,
        prompt_path: &std::path::Path,
        response_path: &std::path::Path,
    ) -> Result<(), AgentError> {
        // Short bootstrap prompt — stays well under Windows 8191-char cmd limit.
        let bootstrap = format!(
            "Read and execute the instructions in {} — write your JSON response to {}",
            prompt_path.display(),
            response_path.display(),
        );
        debug!(bootstrap_len = bootstrap.len(), "bootstrap prompt");

        let mut cmd = build_claude_command(&self.claude_bin);
        cmd.arg("--dangerously-skip-permissions")
            .arg("-p")
            .arg(&bootstrap);

        if let Some(id) = &self.session_id {
            cmd.arg("--resume").arg(id);
        }

        let output = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| AgentError::Spawn(format!("failed to spawn claude: {e}")))?;

        // Extract session_id from stdout if present (--output-format json not used here,
        // but Claude may still print it; we attempt a best-effort parse).
        if let Ok(stdout) = std::str::from_utf8(&output.stdout) {
            if let Some(id) = extract_session_id(stdout) {
                self.session_id = Some(id);
            }
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(AgentError::ProcessFailed(format!(
                "claude exited with {}: stderr={stderr} stdout={stdout}",
                output.status
            )));
        }

        debug!("AgentSession::do_send complete");
        Ok(())
    }

    /// No-op — there is no persistent process to kill.
    pub async fn close(self) {
        info!("AgentSession closed");
    }

    /// Returns the session_id from the last successful send, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Check whether a binary path is executable by asking for its version.
    async fn probe_bin(bin: &PathBuf) -> bool {
        Command::new(bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Build a `tokio::process::Command` for the claude CLI.
///
/// On Windows we invoke `cmd /C claude` so that `.cmd` batch wrappers are
/// resolved correctly by the shell (same approach as zengeld-crawler).
fn build_claude_command(bin: &PathBuf) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(bin);
        cmd
    } else {
        Command::new(bin)
    }
}

/// Try to extract a `session_id` from Claude CLI stdout.
///
/// When run with `--output-format json` Claude prints `{"session_id":"...","result":"..."}`.
/// Without that flag the output is prose; we do a best-effort JSON parse.
fn extract_session_id(stdout: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Partial {
        session_id: String,
    }
    // Try each non-empty line that starts with '{'
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(p) = serde_json::from_str::<Partial>(line) {
                return Some(p.session_id);
            }
        }
    }
    None
}
