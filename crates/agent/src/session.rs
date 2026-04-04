use crawl_core::error::AgentError;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Delimiter written by the session to mark the end of a response.
/// Claude CLI in interactive mode outputs each response followed by a blank line
/// and the prompt string — we use a sentinel we inject instead.
const RESPONSE_SENTINEL: &str = "\x00END_OF_RESPONSE\x00";

/// Default per-send() timeout in seconds.
const DEFAULT_SEND_TIMEOUT_SECS: u64 = 300;

/// A persistent multi-turn Claude CLI process.
///
/// Spawns `claude` once and keeps stdin/stdout open across multiple `send()` calls.
/// This lets the process accumulate conversational context, which is critical for
/// the discovery→validation flow: Phase 1 discovers selectors, Phase 2 validates
/// them — and Phase 2 can reference Phase 1 reasoning without resending the full HTML.
///
/// # Example
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut session = AgentSession::start("claude-sonnet-4-6").await?;
/// let discovery = session.send("Here is the HTML: ... Find selectors.").await?;
/// let validation = session.send("Now validate: does selector div.item work?").await?;
/// session.close().await;
/// # Ok(())
/// # }
/// ```
pub struct AgentSession {
    process: Child,
    stdin: ChildStdin,
    stdout_lines: BufReader<ChildStdout>,
    model: String,
    timeout_secs: u64,
}

impl AgentSession {
    /// Spawn a new persistent claude CLI process.
    ///
    /// The process is started with `--dangerously-skip-permissions` so it can use
    /// tools (Read, etc.) during extraction without prompting for confirmation.
    pub async fn start(model: &str) -> Result<Self, AgentError> {
        Self::start_with_bin(PathBuf::from("claude"), model).await
    }

    /// Same as `start` but with an explicit binary path.
    pub async fn start_with_bin(claude_bin: PathBuf, model: &str) -> Result<Self, AgentError> {
        debug!(model, "Starting persistent AgentSession");

        let mut child = Command::new(&claude_bin)
            .arg("--model")
            .arg(model)
            .arg("--dangerously-skip-permissions")
            .arg("--output-format")
            .arg("stream-json")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| AgentError::Spawn(format!("failed to spawn claude: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::Spawn("claude stdin not available".to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Spawn("claude stdout not available".to_string()))?;

        info!(model, "AgentSession started");

        Ok(Self {
            process: child,
            stdin,
            stdout_lines: BufReader::new(stdout),
            model: model.to_string(),
            timeout_secs: DEFAULT_SEND_TIMEOUT_SECS,
        })
    }

    /// Override the per-send() timeout (default: 300 s).
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Send a prompt and wait for the complete response.
    ///
    /// The session stays alive after this call — the next `send()` will continue
    /// the same conversation thread, with Claude retaining full prior context.
    ///
    /// Each prompt is written to stdin as a single line terminated with the sentinel
    /// instruction. The reader collects stdout lines until it sees `RESPONSE_SENTINEL`.
    pub async fn send(&mut self, prompt: &str) -> Result<String, AgentError> {
        debug!(model = %self.model, prompt_len = prompt.len(), "AgentSession::send");

        // Write the prompt followed by an instruction to emit our sentinel when done.
        let message = format!(
            "{}\n\nWhen you have finished your complete response, output exactly this line on its own: {}\n",
            prompt, RESPONSE_SENTINEL
        );

        timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            self.do_send(&message),
        )
        .await
        .map_err(|_| AgentError::Timeout { secs: self.timeout_secs })?
    }

    async fn do_send(&mut self, message: &str) -> Result<String, AgentError> {
        self.stdin
            .write_all(message.as_bytes())
            .await
            .map_err(|e| AgentError::Spawn(format!("write to claude stdin: {e}")))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| AgentError::Spawn(format!("write newline to claude stdin: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| AgentError::Spawn(format!("flush claude stdin: {e}")))?;

        // Read lines until we see the sentinel or stdout closes.
        let mut response_lines: Vec<String> = Vec::new();
        let mut line = String::new();

        loop {
            line.clear();
            let n = self
                .stdout_lines
                .read_line(&mut line)
                .await
                .map_err(|e| AgentError::Spawn(format!("read from claude stdout: {e}")))?;

            if n == 0 {
                // EOF — process exited unexpectedly.
                warn!("Claude process exited unexpectedly during send()");
                break;
            }

            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

            if trimmed == RESPONSE_SENTINEL {
                break;
            }

            // stream-json format: each line is a JSON event. Collect all text content.
            if let Some(text) = extract_text_from_stream_event(trimmed) {
                response_lines.push(text);
            } else {
                // Not a recognised event — include the raw line so callers can debug.
                if !trimmed.is_empty() {
                    response_lines.push(trimmed.to_string());
                }
            }
        }

        let response = response_lines.join("");
        debug!(response_len = response.len(), "AgentSession::send complete");
        Ok(response)
    }

    /// Kill the claude process and release resources.
    pub async fn close(mut self) {
        drop(self.stdin);
        let _ = self.process.kill().await;
        let _ = self.process.wait().await;
        info!(model = %self.model, "AgentSession closed");
    }

    /// Returns a reference to the model string used by this session.
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Extract human-readable text from a single `stream-json` line emitted by claude CLI.
///
/// The stream-json format emits newline-delimited JSON objects of the form:
/// `{"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}`
///
/// We extract the `text` field from `text_delta` events; all other event types are
/// silently ignored (they carry metadata, not response content).
fn extract_text_from_stream_event(line: &str) -> Option<String> {
    // Fast reject: skip lines that are clearly not our target event type.
    if !line.contains("text_delta") {
        return None;
    }

    let v: serde_json::Value = serde_json::from_str(line).ok()?;

    let delta = v.get("delta")?;
    let delta_type = delta.get("type")?.as_str()?;

    if delta_type != "text_delta" {
        return None;
    }

    delta.get("text")?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_from_text_delta() {
        let line = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello world"}}"#;
        let result = extract_text_from_stream_event(line);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn test_extract_text_ignores_non_text_events() {
        let line = r#"{"type":"message_start","message":{"id":"msg_01","type":"message"}}"#;
        let result = extract_text_from_stream_event(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_text_ignores_input_json_delta() {
        let line = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}"#;
        let result = extract_text_from_stream_event(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_text_empty_line() {
        let result = extract_text_from_stream_event("");
        assert_eq!(result, None);
    }
}
