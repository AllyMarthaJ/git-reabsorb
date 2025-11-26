//! Integration tests for LLM client - requires local `claude` CLI
//!
//! These tests actually invoke the user's local claude CLI to verify
//! the integration works correctly.

use git_scramble::reorganize::llm::{ClaudeCliClient, LlmClient};

#[test]
fn test_claude_cli_exists() {
    // Verify claude CLI is available
    let output = std::process::Command::new("which")
        .arg("claude")
        .output()
        .expect("Failed to run 'which'");

    assert!(
        output.status.success(),
        "claude CLI not found in PATH. Install it or skip these tests."
    );

    let path = String::from_utf8_lossy(&output.stdout);
    println!("claude found at: {}", path.trim());
}

#[test]
fn test_claude_cli_version() {
    // Check claude version/help to verify it runs
    let output = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .expect("Failed to run claude --version");

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("status: {}", output.status);

    // Even if --version isn't supported, we learn something
}

#[test]
fn test_claude_cli_help() {
    // Check claude help to understand available options
    let output = std::process::Command::new("claude")
        .arg("--help")
        .output()
        .expect("Failed to run claude --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== claude --help ===");
    println!("stdout:\n{}", stdout);
    println!("stderr:\n{}", stderr);
    println!("status: {}", output.status);

    // Look for relevant flags
    let combined = format!("{}{}", stdout, stderr);
    if combined.contains("--print") {
        println!("--print flag is supported");
    } else {
        println!("WARNING: --print flag may not be supported");
    }
    if combined.contains("-p") {
        println!("-p flag is supported");
    }
    if combined.contains("stdin") {
        println!("stdin is mentioned in help");
    }
}

#[test]
fn test_claude_cli_simple_prompt() {
    // Try a simple prompt to see if the CLI works at all
    let output = std::process::Command::new("claude")
        .args(["--print", "-p", "Say hello"])
        .output()
        .expect("Failed to run claude");

    println!("=== Simple prompt test ===");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("status: {}", output.status);
}

#[test]
fn test_claude_cli_stdin_echo() {
    // Test if claude accepts stdin input
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("claude")
        .args(["--print"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn claude");

    let prompt = "Say 'test successful'";

    if let Some(mut stdin) = child.stdin.take() {
        match stdin.write_all(prompt.as_bytes()) {
            Ok(()) => println!("Successfully wrote to stdin"),
            Err(e) => println!("Failed to write to stdin: {}", e),
        }
        // Explicitly drop stdin to close it
        drop(stdin);
    }

    let output = child.wait_with_output().expect("Failed to wait for claude");

    println!("=== Stdin test ===");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("status: {}", output.status);
}

#[test]
fn test_claude_cli_with_prompt_flag() {
    // Try using -p flag explicitly instead of stdin
    use std::process::Command;

    let prompt = "Respond with exactly: {\"test\": true}";

    let output = Command::new("claude")
        .args(["--print", "-p", prompt])
        .output()
        .expect("Failed to run claude");

    println!("=== -p flag test ===");
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    println!("status: {}", output.status);
}

#[test]
fn test_claude_client_complete() {
    // Test our actual ClaudeCliClient implementation
    let client = ClaudeCliClient::new();
    let result = client.complete("Say 'hello world'");

    match result {
        Ok(response) => {
            println!("=== ClaudeCliClient.complete() succeeded ===");
            println!("Response: {}", response);
        }
        Err(e) => {
            println!("=== ClaudeCliClient.complete() failed ===");
            println!("Error: {}", e);
            // Don't panic - we want to see the error
        }
    }
}

#[test]
fn test_claude_cli_modes() {
    // Test different invocation modes to find what works
    use std::process::Command;

    let test_cases = vec![
        vec!["--print", "-p", "say hi"],
        vec!["-p", "say hi", "--print"],
        vec!["--print", "--prompt", "say hi"],
        vec!["-p", "say hi"],
        vec!["say hi"],
    ];

    for args in test_cases {
        println!("\n=== Testing: claude {} ===", args.join(" "));

        let output = Command::new("claude").args(&args).output();

        match output {
            Ok(out) => {
                println!("status: {}", out.status);
                println!("stdout: {}", String::from_utf8_lossy(&out.stdout));
                println!("stderr: {}", String::from_utf8_lossy(&out.stderr));
            }
            Err(e) => {
                println!("Failed to execute: {}", e);
            }
        }
    }
}
