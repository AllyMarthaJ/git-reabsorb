//! Generic LLM client infrastructure.
//!
//! This module provides the core traits and implementations for invoking LLMs.
//! Domain-specific prompting and parsing lives in the respective modules
//! (e.g., `reorganize::llm` for commit reorganization, `assessment::llm` for assessment).
//!
//! # Configuration
//!
//! LLM settings can be configured via:
//! - CLI arguments: `--llm-provider`, `--llm-model`
//! - Environment variables: `GIT_REABSORB_LLM_PROVIDER`, `GIT_REABSORB_LLM_MODEL`
//!
//! CLI arguments take precedence over environment variables.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

use log::{debug, trace};

/// Available LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmProvider {
    /// Claude CLI (default)
    #[default]
    Claude,
    /// OpenCode CLI
    OpenCode,
}

impl std::fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::OpenCode => write!(f, "opencode"),
        }
    }
}

impl std::str::FromStr for LlmProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "opencode" => Ok(Self::OpenCode),
            _ => Err(format!(
                "Unknown LLM provider: '{}'. Valid options: claude, opencode",
                s
            )),
        }
    }
}

/// Tool capability sets that can be granted to LLM clients.
///
/// Each capability represents a logical grouping of related tools.
/// The actual tool names differ between providers (Claude vs OpenCode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCapability {
    /// File read/write operations.
    /// - Claude: Read, Write
    /// - OpenCode: read, write, edit
    FileIo,
}

impl ToolCapability {
    /// Convert to Claude CLI tool names.
    pub fn to_claude_tools(self) -> &'static [&'static str] {
        match self {
            Self::FileIo => &["Read", "Write"],
        }
    }

    /// Convert to OpenCode CLI tool names.
    pub fn to_opencode_tools(self) -> &'static [&'static str] {
        match self {
            Self::FileIo => &["read", "write", "edit"],
        }
    }
}

/// Convert a slice of capabilities to tool names for a specific provider.
fn capabilities_to_tools(capabilities: &[ToolCapability], provider: LlmProvider) -> Vec<String> {
    capabilities
        .iter()
        .flat_map(|cap| match provider {
            LlmProvider::Claude => cap.to_claude_tools().iter().copied(),
            LlmProvider::OpenCode => cap.to_opencode_tools().iter().copied(),
        })
        .map(String::from)
        .collect()
}

/// Configuration for LLM clients.
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    /// The LLM provider to use.
    pub provider: LlmProvider,
    /// Optional model override.
    pub model: Option<String>,
    /// Backend provider for opencode (e.g., "lmstudio", "ollama").
    pub opencode_backend: Option<String>,
    /// Tool capabilities to grant the LLM.
    pub capabilities: Option<Vec<ToolCapability>>,
}

impl LlmConfig {
    /// Create a new config with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create config from environment variables.
    ///
    /// Reads:
    /// - `GIT_REABSORB_LLM_PROVIDER` - provider name (claude, opencode)
    /// - `GIT_REABSORB_LLM_MODEL` - model name
    /// - `GIT_REABSORB_OPENCODE_BACKEND` - backend for opencode (e.g., lmstudio, ollama)
    pub fn from_env() -> Self {
        let provider = env::var("GIT_REABSORB_LLM_PROVIDER")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let model = env::var("GIT_REABSORB_LLM_MODEL").ok();
        let opencode_backend = env::var("GIT_REABSORB_OPENCODE_BACKEND").ok();

        Self {
            provider,
            model,
            opencode_backend,
            capabilities: None,
        }
    }

    /// Set the provider.
    pub fn with_provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the opencode backend.
    pub fn with_opencode_backend(mut self, backend: impl Into<String>) -> Self {
        self.opencode_backend = Some(backend.into());
        self
    }

    /// Set tool capabilities for the LLM.
    pub fn with_capabilities(mut self, capabilities: Vec<ToolCapability>) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Merge with CLI overrides. CLI values take precedence.
    pub fn with_overrides(
        mut self,
        provider: Option<LlmProvider>,
        model: Option<String>,
        opencode_backend: Option<String>,
    ) -> Self {
        if let Some(p) = provider {
            self.provider = p;
        }
        if let Some(m) = model {
            self.model = Some(m);
        }
        if let Some(b) = opencode_backend {
            self.opencode_backend = Some(b);
        }
        self
    }

    /// Convert capabilities to tool names for the configured provider.
    fn allowed_tools(&self) -> Option<Vec<String>> {
        self.capabilities
            .as_ref()
            .map(|caps| capabilities_to_tools(caps, self.provider))
    }

    /// Create an LLM client from this configuration.
    pub fn create_client(&self) -> Arc<dyn LlmClient> {
        let allowed_tools = self.allowed_tools();
        match self.provider {
            LlmProvider::Claude => Arc::new(ClaudeCliClient {
                model: self.model.clone(),
                allowed_tools,
            }),
            LlmProvider::OpenCode => Arc::new(OpenCodeClient {
                model: self.model.clone(),
                backend: self.opencode_backend.clone(),
                allowed_tools,
            }),
        }
    }

    /// Create a boxed LLM client from this configuration.
    pub fn create_boxed_client(&self) -> Box<dyn LlmClient> {
        let allowed_tools = self.allowed_tools();
        match self.provider {
            LlmProvider::Claude => Box::new(ClaudeCliClient {
                model: self.model.clone(),
                allowed_tools,
            }),
            LlmProvider::OpenCode => Box::new(OpenCodeClient {
                model: self.model.clone(),
                backend: self.opencode_backend.clone(),
                allowed_tools,
            }),
        }
    }
}

/// Trait for LLM completion clients.
pub trait LlmClient: Send + Sync {
    /// Send a prompt to the LLM and return the completion response.
    fn complete(&self, prompt: &str) -> Result<String, LlmError>;
}

/// Claude CLI client implementation.
pub struct ClaudeCliClient {
    pub model: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
}

impl ClaudeCliClient {
    pub fn new() -> Self {
        Self {
            model: None,
            allowed_tools: None,
        }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
            allowed_tools: None,
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
        // Log the prompt at trace level
        trace!("[claude prompt] -------- START --------");
        for line in prompt.lines() {
            trace!("[claude prompt] {}", line);
        }
        trace!("[claude prompt] -------- END --------");

        // Use stdin for prompt to avoid command line length limits
        let mut args = vec!["--print"];

        let model_str;
        if let Some(ref model) = self.model {
            model_str = model.clone();
            args.push("--model");
            args.push(&model_str);
        }

        // Add allowed tools if specified (comma-separated)
        let tools_str;
        if let Some(ref tools) = self.allowed_tools {
            if !tools.is_empty() {
                tools_str = tools.join(",");
                args.push("--allowedTools");
                args.push(&tools_str);
            }
        }

        let mut child = Command::new("claude")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LlmError::ClientError(format!("Failed to run claude CLI: {}", e)))?;

        // Write prompt to stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .map_err(|e| LlmError::ClientError(format!("Failed to write to stdin: {}", e)))?;
        }

        // Stream output in realtime at trace level (-vv)
        let stream_output = log::log_enabled!(log::Level::Trace);

        if stream_output {
            // Stream stdout and stderr while accumulating the response
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| LlmError::ClientError("Failed to capture stdout".to_string()))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| LlmError::ClientError("Failed to capture stderr".to_string()))?;

            // Spawn thread to read stderr
            let stderr_handle = std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                let mut stderr_output = String::new();
                for line in reader.lines().flatten() {
                    eprintln!("[claude stderr] {}", line);
                    stderr_output.push_str(&line);
                    stderr_output.push('\n');
                }
                stderr_output
            });

            // Read stdout line by line, printing and accumulating
            let reader = BufReader::new(stdout);
            let mut response = String::new();
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        trace!("[claude] {}", line);
                        response.push_str(&line);
                        response.push('\n');
                    }
                    Err(e) => {
                        debug!("Error reading stdout line: {}", e);
                    }
                }
            }

            // Wait for stderr thread
            let _ = stderr_handle.join();

            // Wait for process to finish
            let status = child.wait().map_err(|e| {
                LlmError::ClientError(format!("Failed to wait for claude CLI: {}", e))
            })?;

            if !status.success() {
                return Err(LlmError::ClientError(format!(
                    "claude CLI failed with exit code: {:?}",
                    status.code()
                )));
            }

            Ok(response)
        } else {
            // Buffered mode - wait for all output at once
            let output = child.wait_with_output().map_err(|e| {
                LlmError::ClientError(format!("Failed to wait for claude CLI: {}", e))
            })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Err(LlmError::ClientError(format!(
                    "claude CLI failed: \n\nstderr: {}\n\n stdout: {}",
                    stderr, stdout
                )));
            }

            let response = String::from_utf8_lossy(&output.stdout).to_string();

            // If stderr has content, include it in debug output
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                debug!("Claude CLI stderr: {}", stderr.trim());
            }

            Ok(response)
        }
    }
}

/// OpenCode CLI client implementation.
pub struct OpenCodeClient {
    pub model: Option<String>,
    /// Backend provider (e.g., "lmstudio", "ollama").
    pub backend: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
}

impl OpenCodeClient {
    pub fn new() -> Self {
        Self {
            model: None,
            backend: None,
            allowed_tools: None,
        }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
            backend: None,
            allowed_tools: None,
        }
    }

    pub fn with_backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = Some(backend.into());
        self
    }
}

impl Default for OpenCodeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient for OpenCodeClient {
    fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        // Log the prompt at trace level
        trace!("[opencode prompt] -------- START --------");
        for line in prompt.lines() {
            trace!("[opencode prompt] {}", line);
        }
        trace!("[opencode prompt] -------- END --------");

        // opencode uses: opencode run "prompt" [-m provider/model] --format json
        // Model format is "provider/model" (e.g., "lmstudio/qwen/qwen3-coder-30b")
        let mut args = vec!["run", prompt, "--format", "json"];

        // Build model string in format "backend/model"
        let model_arg;
        match (&self.backend, &self.model) {
            (Some(backend), Some(model)) => {
                // If model already contains a slash, use it as-is under the backend
                // Otherwise combine them
                if model.contains('/') {
                    model_arg = model.clone()
                } else {
                    model_arg = format!("{}/{}", backend, model);
                }
                args.push("-m");
                args.push(&model_arg);
            }
            (Some(_backend), None) => {
                // Just backend without model - this won't work, need a full model path
                // Skip the -m flag and let opencode use defaults
            }
            (None, Some(model)) => {
                // Model specified - use as-is (should be in provider/model format)
                model_arg = model.clone();
                args.push("-m");
                args.push(&model_arg);
            }
            (None, None) => {
                // Use opencode defaults
            }
        }

        // Add allowed tools if specified (comma-separated)
        let tools_str;
        if let Some(ref tools) = self.allowed_tools {
            if !tools.is_empty() {
                tools_str = tools.join(",");
                args.push("--allowedTools");
                args.push(&tools_str);
            }
        }

        let output = Command::new("opencode")
            .args(&args)
            .output()
            .map_err(|e| LlmError::ClientError(format!("Failed to run opencode CLI: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(LlmError::ClientError(format!(
                "opencode CLI failed (exit {}): stderr={} stdout={}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
                stdout.trim()
            )));
        }

        // Parse JSON output - each line is a JSON event, extract text parts
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut text_parts = Vec::new();

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if json.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(text) = json
                        .get("part")
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }

        if text_parts.is_empty() {
            return Err(LlmError::ClientError(format!(
                "No text output from opencode. Raw output: {}",
                stdout.chars().take(500).collect::<String>()
            )));
        }

        Ok(text_parts.join(""))
    }
}

/// Errors from LLM operations.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("LLM client error: {0}")]
    ClientError(String),

    #[error("Failed to parse LLM response: {0}")]
    ParseError(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Invalid ID {0}: not found in input")]
    InvalidId(usize),

    #[error("Invalid index {index} for item {item_id}: out of range")]
    InvalidIndex { item_id: usize, index: usize },

    #[error("Max retries ({0}) exceeded")]
    MaxRetriesExceeded(usize),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Mock LLM client for testing.
#[cfg(test)]
pub mod test_support {
    use super::*;

    pub struct MockLlmClient {
        pub response: String,
    }

    impl MockLlmClient {
        pub fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    impl LlmClient for MockLlmClient {
        fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok(self.response.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_client() {
        let client = test_support::MockLlmClient::new("test response");
        let result = client.complete("test prompt").unwrap();
        assert_eq!(result, "test response");
    }

    #[test]
    fn test_provider_parse() {
        assert_eq!(
            "claude".parse::<LlmProvider>().unwrap(),
            LlmProvider::Claude
        );
        assert_eq!(
            "opencode".parse::<LlmProvider>().unwrap(),
            LlmProvider::OpenCode
        );
        assert_eq!(
            "CLAUDE".parse::<LlmProvider>().unwrap(),
            LlmProvider::Claude
        );
        assert!("unknown".parse::<LlmProvider>().is_err());
    }

    #[test]
    fn test_config_overrides() {
        let config = LlmConfig::new()
            .with_provider(LlmProvider::Claude)
            .with_model("sonnet");

        let updated = config.with_overrides(Some(LlmProvider::OpenCode), None, None);
        assert_eq!(updated.provider, LlmProvider::OpenCode);
        assert_eq!(updated.model, Some("sonnet".to_string()));
        assert_eq!(updated.opencode_backend, None);

        let updated2 = updated.with_overrides(None, Some("gpt-4".to_string()), None);
        assert_eq!(updated2.provider, LlmProvider::OpenCode);
        assert_eq!(updated2.model, Some("gpt-4".to_string()));

        let updated3 = updated2.with_overrides(None, None, Some("lmstudio".to_string()));
        assert_eq!(updated3.provider, LlmProvider::OpenCode);
        assert_eq!(updated3.model, Some("gpt-4".to_string()));
        assert_eq!(updated3.opencode_backend, Some("lmstudio".to_string()));
    }
}
