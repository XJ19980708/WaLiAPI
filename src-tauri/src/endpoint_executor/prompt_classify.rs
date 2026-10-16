//! 按消息分类的 prompt token 统计（可观测性 P1）。
//!
//! 参考 magpie 的分类口径，把一次请求的 prompt 拆成六个分类：
//! - `system`：system/developer 角色消息、Responses 的 `instructions`
//! - `tools`：`tools[]` 工具定义序列化后的体积
//! - `injected`：user 文本中命中注入前缀的（`<system-reminder>`、`<environment_context>`
//!   等 agent 框架注入内容，不是真实用户发言）
//! - `assistant`：assistant 消息（历史对话）
//! - `tool_result`：role=tool / tool 角色的工具输出
//! - `chat`：真实 user 消息
//!
//! 两阶段记账：请求进入时先按 cl100k 本地估算（est_tokens），上游真实 usage
//! 到达后按 `scale = real_prompt / est_prompt` 等比校准（差额补给最大分类）。
//! 校准后的 `tokens` 展示为"实际"，未校准的展示为"估算 ≈"。
//!
//! 只存分类与大小，不存正文——正文沿用 request_body 的落库策略。

use serde_json::Value;
use std::collections::BTreeMap;

use super::estimate_usage::count_tokens;

/// 分类键（与前端展示口径一一对应）。
pub const KIND_SYSTEM: &str = "system";
pub const KIND_TOOLS: &str = "tools";
pub const KIND_INJECTED: &str = "injected";
pub const KIND_ASSISTANT: &str = "assistant";
pub const KIND_TOOL_RESULT: &str = "tool_result";
pub const KIND_CHAT: &str = "chat";

/// 全部分类（固定顺序，供写入时补零行）。
pub const ALL_KINDS: [&str; 6] = [
    KIND_SYSTEM,
    KIND_TOOLS,
    KIND_INJECTED,
    KIND_ASSISTANT,
    KIND_TOOL_RESULT,
    KIND_CHAT,
];

/// 注入内容前缀：user 文本以这些片段开头（或包含于前 200 字符内）时，
/// 判定为 agent 框架注入而非真实用户输入。列表与 magpie 的 injected 前缀
/// 对齐，覆盖 Codex / Claude Code / WorkBuddy 等常见下游。
const INJECTED_MARKERS: [&str; 12] = [
    "<system-reminder>",
    "<environment_context>",
    "<environment-context>",
    "<task-notification>",
    "<system_warning>",
    "<user_instructions>",
    "<permissions",
    "## My request:",
    "The TodoWrite tool hasn't been used recently",
    "The user opened the file",
    "[System Instructions]",
    "<turn-aborted>",
];

/// 一次请求的分类记账结果。
#[derive(Debug, Clone, Default)]
pub struct PromptBreakdown {
    /// kind → (est_tokens, items)
    pub parts: BTreeMap<&'static str, (i64, i64)>,
}

impl PromptBreakdown {
    fn add(&mut self, kind: &'static str, tokens: i64, items: i64) {
        let entry = self.parts.entry(kind).or_insert((0, 0));
        entry.0 += tokens;
        entry.1 += items;
    }

    /// 总估算 prompt token。
    pub fn total_est(&self) -> i64 {
        self.parts.values().map(|(t, _)| t).sum()
    }
}

/// 提取一条消息 content 的文本（string 或 parts 数组的 text 字段）。
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                    text.push('\n');
                }
            }
            text
        }
        _ => String::new(),
    }
}

/// 判断一段 user 文本是否为 agent 注入内容。
fn is_injected(text: &str) -> bool {
    // 按字符边界取前 300 字符（CJK 多字节截断会 panic，用 char_indices）。
    let head: String = text
        .char_indices()
        .take_while(|(idx, _)| *idx < 300)
        .map(|(_, ch)| ch)
        .collect();
    INJECTED_MARKERS.iter().any(|marker| head.contains(marker))
}

/// 对请求体做分类估算。覆盖三种下游协议：
/// - Chat Completions / Anthropic Messages：`messages[]` + `tools[]`
/// - Responses：`input`（string 或数组）+ `tools[]`
/// - 兜底：整个 JSON body 计入 `chat`
pub fn classify_prompt(body: &Value) -> PromptBreakdown {
    let mut breakdown = PromptBreakdown::default();

    // 工具定义：序列化后计数（含 schema 体积）。
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        if !tools.is_empty() {
            let text = serde_json::to_string(tools).unwrap_or_default();
            let tokens = count_tokens(&text) + (tools.len() as i64) * 4;
            breakdown.add(KIND_TOOLS, tokens, tools.len() as i64);
        }
    }

    // Responses 的 instructions 字段等价于 system。
    if let Some(instructions) = body.get("instructions").and_then(Value::as_str) {
        if !instructions.is_empty() {
            breakdown.add(KIND_SYSTEM, count_tokens(instructions) + 4, 1);
        }
    }

    // messages 数组（Chat / Anthropic）。
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for msg in messages {
            let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
            let text = content_text(msg.get("content").unwrap_or(&Value::Null));
            let tokens = count_tokens(&text) + 4;
            match role {
                "system" | "developer" => breakdown.add(KIND_SYSTEM, tokens, 1),
                "assistant" => breakdown.add(KIND_ASSISTANT, tokens, 1),
                "tool" | "function" => breakdown.add(KIND_TOOL_RESULT, tokens, 1),
                "user" => {
                    if is_injected(&text) {
                        breakdown.add(KIND_INJECTED, tokens, 1);
                    } else {
                        breakdown.add(KIND_CHAT, tokens, 1);
                    }
                }
                _ => breakdown.add(KIND_CHAT, tokens, 1),
            }
        }
    }

    // Responses 的 input（messages 缺失或为空时才走这里；instructions 不算消息）。
    let has_messages = body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|m| !m.is_empty());
    if !has_messages {
        if let Some(input) = body.get("input") {
            match input {
                Value::String(s) => {
                    let tokens = count_tokens(s) + 4;
                    if is_injected(s) {
                        breakdown.add(KIND_INJECTED, tokens, 1);
                    } else {
                        breakdown.add(KIND_CHAT, tokens, 1);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        let role = item.get("role").and_then(Value::as_str).unwrap_or("");
                        let text = content_text(item)
                            + item.get("output").and_then(Value::as_str).unwrap_or("");
                        let tokens = count_tokens(&text) + 4;
                        match item.get("type").and_then(Value::as_str).unwrap_or(role) {
                            "function_call_output" | "tool_result" => {
                                breakdown.add(KIND_TOOL_RESULT, tokens, 1)
                            }
                            t if t.contains("reasoning") => {
                                breakdown.add(KIND_ASSISTANT, tokens, 1)
                            }
                            _ => match role {
                                "assistant" => breakdown.add(KIND_ASSISTANT, tokens, 1),
                                "system" | "developer" => breakdown.add(KIND_SYSTEM, tokens, 1),
                                _ => {
                                    if is_injected(&text) {
                                        breakdown.add(KIND_INJECTED, tokens, 1);
                                    } else {
                                        breakdown.add(KIND_CHAT, tokens, 1);
                                    }
                                }
                            },
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // 全空兜底：整个 body 计入 chat（与 estimate_usage 的兜底口径一致）。
    if breakdown.parts.values().map(|(_, n)| n).sum::<i64>() == 0 {
        let json_str = serde_json::to_string(body).unwrap_or_default();
        breakdown.add(KIND_CHAT, count_tokens(&json_str), 1);
    }

    breakdown
}

/// 校准：上游真实 prompt tokens 到达后，按比例缩放各分类。
/// 差额（真实 - 缩放和）补给最大分类，保证 sum(tokens) == real_prompt。
/// 估算总量为 0 时（理论不可能，classify_prompt 有兜底）直接平分。
pub fn calibrate(
    breakdown: &PromptBreakdown,
    real_prompt: i64,
) -> Vec<crate::db::models::NewLogPromptPart> {
    let est_total = breakdown.total_est();
    let scale = if est_total > 0 {
        real_prompt as f64 / est_total as f64
    } else {
        0.0
    };

    let mut rows: Vec<(String, i64, i64, i64)> = breakdown
        .parts
        .iter()
        .map(|(kind, (est, items))| {
            let calibrated = ((*est as f64) * scale).round() as i64;
            (kind.to_string(), calibrated, *est, *items)
        })
        .collect();
    // 补零行：保证六分类始终齐全，前端渲染稳定。
    for kind in ALL_KINDS {
        if !rows.iter().any(|(k, _, _, _)| k == kind) {
            rows.push((kind.to_string(), 0, 0, 0));
        }
    }

    // 差额补给最大分类，保证 sum(tokens) == real_prompt。
    let sum: i64 = rows.iter().map(|(_, t, _, _)| t).sum();
    let diff = real_prompt - sum;
    if diff != 0 {
        let max_idx = rows
            .iter()
            .enumerate()
            .filter(|(_, (_, tokens, _, _))| *tokens > 0)
            .max_by_key(|(_, (_, tokens, _, _))| *tokens)
            .map(|(idx, _)| idx);
        if let Some(idx) = max_idx {
            rows[idx].1 = (rows[idx].1 + diff).max(0);
        }
    }

    rows.into_iter()
        .map(
            |(kind, tokens, est, items)| crate::db::models::NewLogPromptPart {
                log_id: String::new(),
                kind,
                items,
                tokens,
                est_tokens: est,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_system_tools_and_chat() {
        let body = json!({
            "model": "gpt-4o",
            "tools": [{"type": "function", "function": {"name": "read_file", "parameters": {}}}],
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello, what is Rust?"}
            ]
        });
        let breakdown = classify_prompt(&body);
        assert!(breakdown.parts.get(KIND_SYSTEM).unwrap().0 > 0);
        assert!(breakdown.parts.get(KIND_TOOLS).unwrap().0 > 0);
        assert_eq!(breakdown.parts.get(KIND_TOOLS).unwrap().1, 1);
        assert!(breakdown.parts.get(KIND_CHAT).unwrap().0 > 0);
        assert!(breakdown.total_est() > 0);
    }

    #[test]
    fn classifies_injected_user_content() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user", "content": "<system-reminder>This is injected context</system-reminder>"},
                {"role": "user", "content": "Really my question"}
            ]
        });
        let breakdown = classify_prompt(&body);
        assert!(breakdown.parts.get(KIND_INJECTED).unwrap().0 > 0);
        assert!(breakdown.parts.get(KIND_CHAT).unwrap().0 > 0);
    }

    #[test]
    fn classifies_tool_results_and_assistant_history() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user", "content": "read the file"},
                {"role": "assistant", "content": "let me check"},
                {"role": "tool", "content": "file contents here"}
            ]
        });
        let breakdown = classify_prompt(&body);
        assert!(breakdown.parts.get(KIND_ASSISTANT).unwrap().0 > 0);
        assert!(breakdown.parts.get(KIND_TOOL_RESULT).unwrap().0 > 0);
    }

    #[test]
    fn calibrate_scales_and_preserves_total() {
        let mut breakdown = PromptBreakdown::default();
        breakdown.add(KIND_SYSTEM, 800, 1);
        breakdown.add(KIND_CHAT, 200, 1);
        let rows = calibrate(&breakdown, 1000);
        let sum: i64 = rows.iter().map(|r| r.tokens).sum();
        assert_eq!(sum, 1000, "校准后各分类之和必须等于真实 prompt");
        // 4:1 比例应大致保留
        let system = rows.iter().find(|r| r.kind == KIND_SYSTEM).unwrap();
        let chat = rows.iter().find(|r| r.kind == KIND_CHAT).unwrap();
        assert!((system.tokens - 800).abs() <= 10);
        assert!((chat.tokens - 200).abs() <= 10);
    }

    #[test]
    fn calibrate_fills_all_kinds() {
        let mut breakdown = PromptBreakdown::default();
        breakdown.add(KIND_CHAT, 100, 1);
        let rows = calibrate(&breakdown, 100);
        assert_eq!(rows.len(), ALL_KINDS.len(), "六个分类必须齐全（含零行）");
    }

    #[test]
    fn responses_input_is_classified() {
        let body = json!({
            "model": "gpt-5",
            "instructions": "be brief",
            "input": [
                {"type": "message", "role": "user", "content": "hello there"},
                {"type": "function_call_output", "call_id": "c1", "output": "{\"result\": \"done\"}"}
            ]
        });
        let breakdown = classify_prompt(&body);
        assert!(breakdown.parts.get(KIND_SYSTEM).unwrap().0 > 0);
        assert!(breakdown.parts.get(KIND_CHAT).unwrap().0 > 0);
        assert!(breakdown.parts.get(KIND_TOOL_RESULT).unwrap().0 > 0);
    }

    #[test]
    fn cjk_text_does_not_panic_on_char_boundary() {
        // 回归：中文文本按字节截断会切进多字节字符内部（panic）。
        let body = json!({
            "messages": [
                {"role": "user", "content": "你是检索结果重排器。根据查询对候选片段按相关性从高到低排序。查询：alpha 候选片段"}
            ]
        });
        let breakdown = classify_prompt(&body);
        assert!(breakdown.parts.get(KIND_CHAT).unwrap().0 > 0);
    }
}
