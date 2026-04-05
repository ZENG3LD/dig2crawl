//! L4 captcha solver stub.
//!
//! Architecture placeholder only — not planned for implementation unless
//! a specific high-value target requires it.
//!
//! External services to evaluate when needed: 2captcha.com, anti-captcha.com, CapSolver.

use std::future::Future;

/// A captcha challenge detected on a page.
#[derive(Debug, Clone)]
pub struct CaptchaChallenge {
    /// Provider name: "recaptcha", "hcaptcha", "turnstile", "cloudflare", etc.
    pub provider: String,
    /// Site key if extractable from the page HTML.
    pub site_key: Option<String>,
    /// URL of the page where the captcha was encountered.
    pub page_url: String,
}

/// Trait for captcha solver integrations.
///
/// Not planned for implementation — stub only. Exists to give the escalation
/// coordinator a typed endpoint to return `CaptchaError::NotImplemented`
/// rather than a panic.
pub trait CaptchaSolver: Send + Sync {
    fn solve(
        &self,
        challenge: &CaptchaChallenge,
    ) -> impl Future<Output = Result<String, CaptchaError>> + Send;
}

/// Captcha-related errors.
#[derive(Debug, thiserror::Error)]
pub enum CaptchaError {
    #[error("captcha solving not implemented (provider: {provider})")]
    NotImplemented { provider: String },
    #[error("external solver failed: {0}")]
    SolverFailed(String),
    #[error("solver timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u32 },
}

/// No-op solver that always returns `CaptchaError::NotImplemented`.
pub struct NoOpSolver;

impl CaptchaSolver for NoOpSolver {
    async fn solve(&self, challenge: &CaptchaChallenge) -> Result<String, CaptchaError> {
        Err(CaptchaError::NotImplemented {
            provider: challenge.provider.clone(),
        })
    }
}
