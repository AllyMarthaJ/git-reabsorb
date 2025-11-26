//! LLM client implementations

use std::process::Command;

use super::types::LlmError;

/// Trait for LLM completion
pub trait LlmClient {
    /// Send a prompt and get a completion response
    fn complete(&self, prompt: &str) -> Result<String, LlmError>;
}

/// Client that invokes the local `claude` CLI
pub struct ClaudeCliClient {
    /// Model to use (e.g., "sonnet", "opus")
    pub model: Option<String>,
}

impl ClaudeCliClient {
    pub fn new() -> Self {
        Self { model: None }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
        }
    }
}

impl Default for ClaudeCliClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient for ClaudeCliClient {
    fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        // Invoke: claude --print -p "prompt"
        let mut args = vec!["--print", "-p", prompt];

        let model_str;
        if let Some(ref model) = self.model {
            model_str = model.clone();
            args.push("--model");
            args.push(&model_str);
        }

        let output = Command::new("claude")
            .args(&args)
            .output()
            .map_err(|e| LlmError::ClientError(format!("Failed to run claude CLI: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LlmError::ClientError(format!(
                "claude CLI failed: {}",
                stderr
            )));
        }

        let response = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock client for testing
    #[allow(dead_code)]
    pub struct MockLlmClient {
        pub response: String,
    }

    #[allow(dead_code)]
    impl LlmClient for MockLlmClient {
        fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok(self.response.clone())
        }
    }
}
