//! Level 3 visual extraction — screenshot → Claude Vision → coordinate-based actions.

use crate::agent::actions::VisualAction;
use crate::agent::session::AgentSession;
use std::path::Path;

/// Drives a single L3 visual extraction turn.
pub struct VisualExtractionSession<'a> {
    session: &'a mut AgentSession,
    job_dir: &'a Path,
}

impl<'a> VisualExtractionSession<'a> {
    pub fn new(session: &'a mut AgentSession, job_dir: &'a Path) -> Self {
        Self { session, job_dir }
    }

    /// Send a screenshot to Claude and get back a list of visual actions.
    pub async fn analyze(
        &mut self,
        screenshot_png: &[u8],
        goal: &str,
        html_hint: &str,
    ) -> Result<Vec<VisualAction>, anyhow::Error> {
        use anyhow::Context;

        // Write screenshot to temp file
        let screenshot_path = self.job_dir.join("screenshot_l3.png");
        tokio::fs::write(&screenshot_path, screenshot_png)
            .await
            .with_context(|| format!("Failed to write L3 screenshot to {}", screenshot_path.display()))?;

        // Build visual prompt
        let response_path = self.job_dir.join("response_l3.json");
        let prompt = crate::agent::prompts::build_visual_prompt(
            &screenshot_path,
            &response_path,
            goal,
            html_hint,
        );

        // Write prompt to file
        let prompt_path = self.job_dir.join("prompt_l3.md");
        tokio::fs::write(&prompt_path, &prompt)
            .await
            .with_context(|| format!("Failed to write L3 prompt to {}", prompt_path.display()))?;

        // Send to Claude (same session — Claude has L1/L2 context)
        self.session
            .send_with_files(&prompt_path, &response_path)
            .await
            .context("L3 visual prompt failed")?;

        // Read response
        let response_raw = tokio::fs::read_to_string(&response_path)
            .await
            .with_context(|| format!("Claude did not write L3 response to {}", response_path.display()))?;

        // Parse — try direct JSON first, then extract from ```json ... ``` fences
        let json_str = extract_json_from_response(&response_raw);
        let response: serde_json::Value = serde_json::from_str(&json_str).unwrap_or_default();

        let visual_actions: Vec<VisualAction> = response
            .get("visual_actions")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(visual_actions)
    }
}

/// Try to extract a JSON object from a Claude response string.
///
/// Attempts direct parse first. Falls back to scanning for a ` ```json ` fenced
/// block, then to the first `{`…`}` span in the string.
fn extract_json_from_response(raw: &str) -> String {
    let trimmed = raw.trim();

    // Fast path: the whole response is already valid JSON.
    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }

    // Try ````json ... ```` fenced block.
    if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim().to_string();
        }
    }

    // Try plain ``` ... ``` fenced block.
    if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        if let Some(end) = after_fence.find("```") {
            let candidate = after_fence[..end].trim();
            if candidate.starts_with('{') {
                return candidate.to_string();
            }
        }
    }

    // Last resort: find first '{' and last '}'.
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            return trimmed[start..=end].to_string();
        }
    }

    // Return as-is and let the caller's serde_json::from_str produce an error.
    trimmed.to_string()
}
