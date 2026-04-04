use crate::prompts::AGENT_SYSTEM_PROMPT;
use crate::protocol::{AgentRequest, AgentResponse};
use crawl_core::error::AgentError;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, warn};

/// Spawns a claude CLI process for a single extraction task.
pub struct ClaudeSpawner {
    pub claude_bin: PathBuf,
    pub model: String,
    pub timeout_secs: u64,
}

impl ClaudeSpawner {
    pub fn new() -> Self {
        Self {
            claude_bin: PathBuf::from("claude"),
            model: "claude-sonnet-4-6".to_string(),
            timeout_secs: 120,
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Serialise `request` to disk, invoke the claude CLI, parse the response.
    pub async fn invoke(
        &self,
        request: &AgentRequest,
        work_dir: &Path,
    ) -> Result<AgentResponse, AgentError> {
        // Write request.json to work_dir so the agent can reference it.
        let request_path = work_dir.join("request.json");
        let request_json = serde_json::to_string_pretty(request)
            .map_err(|e| AgentError::Spawn(format!("serialize request: {e}")))?;
        tokio::fs::write(&request_path, &request_json)
            .await
            .map_err(|e| AgentError::Spawn(format!("write request.json: {e}")))?;

        // Build the full prompt: system instructions + the task JSON.
        let prompt = format!(
            "{}\n\n---\n\nHere is your extraction task:\n\n{}",
            AGENT_SYSTEM_PROMPT, request_json,
        );

        debug!(task_id = %request.task_id, "Spawning claude agent");

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            Command::new(&self.claude_bin)
                .arg("-p")
                .arg(&prompt)
                .arg("--model")
                .arg(&self.model)
                .arg("--output-format")
                .arg("json")
                .arg("--dangerously-skip-permissions")
                .current_dir(work_dir)
                .output(),
        )
        .await
        .map_err(|_| AgentError::Timeout { secs: self.timeout_secs })?
        .map_err(|e| AgentError::Spawn(format!("execute claude: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(task_id = %request.task_id, %stderr, "Claude process failed");
            return Err(AgentError::ProcessFailed(format!(
                "exit {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_response(&stdout)
    }

    fn parse_response(stdout: &str) -> Result<AgentResponse, AgentError> {
        // Try parsing stdout directly as AgentResponse.
        if let Ok(resp) = serde_json::from_str::<AgentResponse>(stdout) {
            return Ok(resp);
        }

        // Claude --output-format json wraps content in an envelope with a "result" field.
        #[derive(serde::Deserialize)]
        struct ClaudeEnvelope {
            result: Option<String>,
        }

        if let Ok(envelope) = serde_json::from_str::<ClaudeEnvelope>(stdout) {
            if let Some(result_str) = envelope.result {
                if let Ok(resp) = serde_json::from_str::<AgentResponse>(&result_str) {
                    return Ok(resp);
                }
                // The result might embed the JSON inside prose — find the outermost object.
                if let Some(start) = result_str.find('{') {
                    if let Some(end) = result_str.rfind('}') {
                        let json_slice = &result_str[start..=end];
                        if let Ok(resp) = serde_json::from_str::<AgentResponse>(json_slice) {
                            return Ok(resp);
                        }
                    }
                }
            }
        }

        Err(AgentError::ParseResponse(format!(
            "failed to parse agent response: {}",
            stdout.chars().take(500).collect::<String>()
        )))
    }
}

impl Default for ClaudeSpawner {
    fn default() -> Self {
        Self::new()
    }
}
