//! 调用来源识别：从下游请求头推断接入软件的友好名称。
//!
//! 识别优先级：显式标记头 > User-Agent 特征 > 兜底类别。
//! 结果只用于审计日志展示（`request_logs.client_name`），不参与任何路由或
//! 鉴权决策——UA 可被调用方任意伪造，这里只做"尽力而为"的归因。

/// 下游可在请求头里显式声明接入软件名的标记头（小写）。
/// 与 User-Agent 相比优先级更高，便于集成方自助标注准确来源。
pub const CLIENT_NAME_HEADER: &str = "x-wali-client";

/// 从下游请求头识别调用来源的友好名称。
///
/// - `x-wali-client: Foo` → `Foo`（显式声明，最可信，超长截断）
/// - User-Agent 命中已知特征 → 规范名称（如 "Claude Code"）
/// - 其余 UA → 取产品名首段（截断）
/// - 无 UA → `None`（前端展示为 "-"）
pub fn detect_client_name(user_agent: Option<&str>, client_header: Option<&str>) -> Option<String> {
    if let Some(explicit) = client_header.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(truncate(explicit));
    }
    let ua = user_agent.map(str::trim).filter(|s| !s.is_empty())?;
    if let Some(name) = known_client(ua) {
        return Some(name.to_string());
    }
    // 兜底：取 UA 首段产品名（到第一个空格或 `/` 为止）。
    let head = ua.split([' ', '/']).next().unwrap_or(ua);
    if head.is_empty() {
        return None;
    }
    Some(truncate(head))
}

/// 便捷封装：直接从 Axum 请求头识别。
pub fn detect_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let ua = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let explicit = headers
        .get(CLIENT_NAME_HEADER)
        .and_then(|v| v.to_str().ok());
    detect_client_name(ua, explicit)
}

/// 已知接入软件的 UA 特征 → 规范显示名。按特征在 UA 中出现的特异性排序，
/// 先匹配更具体的（如 "claude-code" 先于裸 "claude"）。
fn known_client(ua_lower: &str) -> Option<&'static str> {
    const RULES: &[(&str, &str)] = &[
        ("claude-code", "Claude Code"),
        ("claudecode", "Claude Code"),
        ("claude/", "Claude"),
        ("codex", "Codex"),
        ("openai-codex", "Codex"),
        ("workbuddy", "WorkBuddy"),
        ("codebuddy", "CodeBuddy"),
        ("cursor", "Cursor"),
        ("windsurf", "Windsurf"),
        ("trae", "Trae"),
        ("vscode", "VS Code"),
        ("visual-studio-code", "VS Code"),
        ("jetbrains", "JetBrains"),
        ("intellij", "JetBrains"),
        ("goose", "Goose"),
        ("cline", "Cline"),
        ("roo-code", "Roo Code"),
        ("roocode", "Roo Code"),
        ("continue", "Continue"),
        ("opencode", "OpenCode"),
        ("gemini-cli", "Gemini CLI"),
        ("curl", "curl"),
        ("python-requests", "Python Requests"),
        ("python-httpx", "Python httpx"),
        ("okhttp", "OkHttp"),
        ("axios", "Axios"),
        ("node-fetch", "Node Fetch"),
        ("undici", "Undici"),
        ("apache-httpclient", "Apache HttpClient"),
    ];
    RULES
        .iter()
        .find(|(needle, _)| ua_lower.contains(needle))
        .map(|(_, name)| *name)
}

fn truncate(name: &str) -> String {
    // 按 char 截断到 60 字符，避免异常长的 UA 撑爆列宽。
    name.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_header_wins() {
        assert_eq!(
            detect_client_name(Some("curl/8.0"), Some("MyApp")),
            Some("MyApp".to_string())
        );
    }

    #[test]
    fn known_ua_patterns() {
        assert_eq!(
            detect_client_name(Some("claude-code/1.2.3 (darwin)"), None),
            Some("Claude Code".to_string())
        );
        assert_eq!(
            detect_client_name(Some("WorkBuddy/0.3"), None),
            Some("WorkBuddy".to_string())
        );
    }

    #[test]
    fn fallback_takes_product_head() {
        assert_eq!(
            detect_client_name(Some("some-tool/2.0 (compatible)"), None),
            Some("some-tool".to_string())
        );
    }

    #[test]
    fn empty_inputs_give_none() {
        assert_eq!(detect_client_name(None, None), None);
        assert_eq!(detect_client_name(Some(""), Some("")), None);
    }

    #[test]
    fn long_name_truncated() {
        let long = "x".repeat(200);
        let got = detect_client_name(Some(&long), None).unwrap();
        assert_eq!(got.chars().count(), 60);
    }
}
