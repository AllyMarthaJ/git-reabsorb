use crate::models::DiffLine;

/// Truncate a SHA to its first 8 characters for display
pub fn short_sha(sha: &str) -> &str {
    &sha[..8.min(sha.len())]
}

/// Format diff lines with standard +/- prefixes for display
pub fn format_diff_lines(lines: &[DiffLine]) -> String {
    lines
        .iter()
        .map(|line| match line {
            DiffLine::Context(s) => format!(" {}", s),
            DiffLine::Added(s) => format!("+{}", s),
            DiffLine::Removed(s) => format!("-{}", s),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract JSON content from an LLM response.
///
/// Handles three formats:
/// 1. JSON in a ```json code fence
/// 2. JSON in a generic ``` code fence
/// 3. Raw JSON starting with `{`
///
/// Returns the extracted JSON string slice, or None if no JSON found.
pub fn extract_json_str(response: &str) -> Option<&str> {
    // Try ```json fence
    if let Some(start) = response.find("```json") {
        let content_start = start + 7;
        let end = response[content_start..]
            .find("```")
            .map(|e| content_start + e)?;
        return Some(response[content_start..end].trim());
    }

    // Try generic ``` fence
    if let Some(start) = response.find("```") {
        let content_start = start + 3;
        // Skip language identifier on same line
        let line_end = response[content_start..]
            .find('\n')
            .map(|n| content_start + n + 1)
            .unwrap_or(content_start);
        let end = response[line_end..].find("```").map(|e| line_end + e)?;
        return Some(response[line_end..end].trim());
    }

    // Try raw JSON
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    if start <= end {
        Some(response[start..=end].trim())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_sha() {
        assert_eq!(short_sha("abc123def456"), "abc123de");
        assert_eq!(short_sha("short"), "short");
        assert_eq!(short_sha(""), "");
    }

    #[test]
    fn test_format_diff_lines() {
        let lines = vec![
            DiffLine::Context("unchanged".to_string()),
            DiffLine::Added("new line".to_string()),
            DiffLine::Removed("old line".to_string()),
        ];
        let formatted = format_diff_lines(&lines);
        assert!(formatted.contains(" unchanged"));
        assert!(formatted.contains("+new line"));
        assert!(formatted.contains("-old line"));
    }

    #[test]
    fn test_extract_json_str_code_fence() {
        let response = r#"Here's the JSON:
```json
{"key": "value"}
```
That's it!"#;
        assert_eq!(extract_json_str(response), Some(r#"{"key": "value"}"#));
    }

    #[test]
    fn test_extract_json_str_generic_fence() {
        let response = r#"```
{"key": "value"}
```"#;
        assert_eq!(extract_json_str(response), Some(r#"{"key": "value"}"#));
    }

    #[test]
    fn test_extract_json_str_raw() {
        let response = r#"The result is {"key": "value"} here"#;
        assert_eq!(extract_json_str(response), Some(r#"{"key": "value"}"#));
    }

    #[test]
    fn test_extract_json_str_none() {
        assert_eq!(extract_json_str("no json here"), None);
    }

    #[test]
    fn test_extract_json_str_with_banner() {
        let response = "Running node v24.8.0 (npm v11.6.0)\n{\"key\": \"value\"}";
        assert_eq!(extract_json_str(response), Some(r#"{"key": "value"}"#));
    }

    #[test]
    fn test_extract_json_str_banner_only() {
        let response = "Running node v24.8.0 (npm v11.6.0)";
        assert_eq!(extract_json_str(response), None);
    }
}
