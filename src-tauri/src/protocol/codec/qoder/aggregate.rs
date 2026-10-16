//! Qoder SSE → OpenAI Chat 完成体的聚合器（非流式路径用）。
//!
//! Qoder 网关对**任何**请求（包括 `stream:false`）都返回 SSE
//! `text/event-stream`：每条记录是信封 `{headers, body, statusCodeValue}`，
//! `body` 是转义的内层 OpenAI Chat chunk JSON 字符串。本聚合器把整条流
//! 还原成一个完整的 `chat.completion` 对象，供非流式解码器（
//! [`super::QoderThenChatTo`]）消费。
//!
//! 纯计算模块：不发请求、不碰凭据。

use serde_json::{json, Value};

/// 聚合状态。
#[derive(Default)]
pub struct QoderChatAggregator {
    request_id: Option<String>,
    model: Option<String>,
    created: Option<i64>,
    content: String,
    reasoning: String,
    tool_calls: Vec<Value>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    failed: Option<String>,
    pending: Vec<u8>,
    /// 是否收到过至少一个有效内层事件（信封解析成功的 chunk）。
    /// 截断容错的判据：有事件 → 容忍尾部残片；完全没有 → 报错。
    saw_event: bool,
}

impl QoderChatAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段上游字节。记录边界由 SSE 空行分隔。
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > 32 * 1024 * 1024 {
            return Err("Qoder SSE pending buffer overflow".to_owned());
        }
        while let Some(end) = sse_record_end(&self.pending) {
            let record: Vec<u8> = self.pending.drain(..end).collect();
            self.record(&record)?;
        }
        Ok(())
    }

    fn record(&mut self, record: &[u8]) -> Result<(), String> {
        let payload = sse_data_payload(record)?;
        if payload.is_empty() || payload == "[DONE]" {
            return Ok(());
        }
        let event: Value =
            serde_json::from_str(&payload).map_err(|e| format!("Qoder SSE JSON invalid: {e}"))?;
        // 信封：{"headers":..., "body":"<json string>", "statusCodeValue":200}
        let status = event
            .get("statusCodeValue")
            .and_then(Value::as_u64)
            .unwrap_or(200);
        if status >= 400 {
            self.failed = Some(format!("Qoder gateway envelope status {status}"));
            return Ok(());
        }
        let inner = match event.get("body") {
            Some(Value::String(text)) if text == "[DONE]" || text.is_empty() => return Ok(()),
            Some(Value::String(text)) => serde_json::from_str::<Value>(text)
                .map_err(|e| format!("Qoder envelope body JSON invalid: {e}"))?,
            Some(other @ Value::Object(_)) => other.clone(),
            _ => return Ok(()),
        };
        // 信封解析成功即视为收到过有效事件（截断容错判据）。
        self.saw_event = true;
        if let Some(id) = inner.get("id").and_then(Value::as_str) {
            self.request_id.get_or_insert_with(|| id.to_owned());
        }
        if let Some(model) = inner.get("model").and_then(Value::as_str) {
            self.model.get_or_insert_with(|| model.to_owned());
        }
        if let Some(created) = inner.get("created").and_then(Value::as_i64) {
            self.created.get_or_insert(created);
        }
        if let Some(usage) = inner.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(usage.clone());
        }
        for choice in inner
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let delta = choice.get("delta").or_else(|| choice.get("message"));
            let Some(delta) = delta else { continue };
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                self.content.push_str(text);
            }
            if let Some(text) = delta.get("reasoning_content").and_then(Value::as_str) {
                self.reasoning.push_str(text);
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    merge_tool_call(&mut self.tool_calls, call);
                }
            }
            if let Some(reason) = choice
                .get("finish_reason")
                .filter(|r| !r.is_null())
                .and_then(Value::as_str)
            {
                self.finish_reason = Some(reason.to_owned());
            }
        }
        Ok(())
    }

    /// 输出完整 `chat.completion`。流必须已 EOF。
    ///
    /// 容错策略：Qoder 网关（或中间代理）可能在最后一条记录未带空行终止符时
    /// 就关闭连接（长生成/慢生成时更常见）。`finish` 会先把残留字节按"最后
    /// 一条不完整记录"尽力解析；收到过有效事件时容忍尾部残片（不完整记录
    /// 通常是纯遥测如 `event:finish` 或被截断的收尾 chunk），完全没收到过
    /// 事件才报错（不能把失败伪装成空回复）。
    pub fn finish(mut self) -> Result<Value, String> {
        if !self.pending.is_empty() {
            // 尽力解析残留的半条记录；失败不影响已有聚合结果。
            let residue = std::mem::take(&mut self.pending);
            let _ = self.record(&residue);
        }
        if let Some(failed) = self.failed.take() {
            return Err(failed);
        }
        if !self.saw_event {
            return Err("Qoder SSE ended before any content event".to_owned());
        }
        let finish_reason = self.finish_reason.unwrap_or_else(|| {
            if self.tool_calls.is_empty() {
                "stop"
            } else {
                "tool_calls"
            }
            .to_owned()
        });
        let mut message = json!({
            "role": "assistant",
            "content": if self.content.is_empty() { Value::Null } else { Value::String(self.content.clone()) },
        });
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = json!(self.reasoning);
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(self.tool_calls.clone());
        }
        let usage = self.usage.clone().unwrap_or_else(
            || json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}),
        );
        Ok(json!({
            "id": self.request_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())),
            "object": "chat.completion",
            "created": self.created.unwrap_or_else(|| chrono::Utc::now().timestamp()),
            "model": self.model.clone().unwrap_or_default(),
            "choices": [{
                "index": 0,
                "message": message,
                "finish_reason": finish_reason,
            }],
            "usage": usage,
        }))
    }
}

/// 增量合并 tool call delta（按 index 合并 arguments 片段）。
fn merge_tool_call(tool_calls: &mut Vec<Value>, delta: &Value) {
    let index = delta
        .get("index")
        .and_then(Value::as_u64)
        .unwrap_or(tool_calls.len() as u64);
    while tool_calls.len() <= index as usize {
        tool_calls.push(json!({
            "id": format!("call_{}", uuid::Uuid::new_v4().simple()),
            "type": "function",
            "function": {"name": "", "arguments": ""}
        }));
    }
    let slot = &mut tool_calls[index as usize];
    if let Some(id) = delta.get("id").and_then(Value::as_str) {
        if !id.is_empty() {
            slot["id"] = json!(id);
        }
    }
    if let Some(function) = delta.get("function") {
        if let Some(name) = function.get("name").and_then(Value::as_str) {
            if !name.is_empty() {
                slot["function"]["name"] = json!(name);
            }
        }
        if let Some(args) = function.get("arguments").and_then(Value::as_str) {
            let existing = slot["function"]["arguments"].as_str().unwrap_or("");
            slot["function"]["arguments"] = json!(format!("{existing}{args}"));
        }
    }
}

/// SSE 记录边界：空行（\n\n 或 \r\n\r\n）结尾。返回记录总长度（含边界）。
fn sse_record_end(buffer: &[u8]) -> Option<usize> {
    let mut index = 0;
    while index + 1 < buffer.len() {
        if buffer[index] == b'\n' {
            if buffer[index + 1] == b'\n' {
                return Some(index + 2);
            }
            if buffer[index + 1] == b'\r' && index + 3 < buffer.len() && buffer[index + 2] == b'\n'
            {
                return Some(index + 3);
            }
        }
        if buffer[index] == b'\r' && buffer[index + 1] == b'\r' {
            return Some(index + 2);
        }
        index += 1;
    }
    None
}

/// 提取一条 SSE 记录的 data 载荷（多行 data 拼接）。
fn sse_data_payload(record: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(record).map_err(|_| "SSE record is not UTF-8".to_owned())?;
    let mut data = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // event:/id:/注释行忽略；Qoder 只用 data。
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = concat!(
        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"Hello\\\",\\\"role\\\":\\\"assistant\\\"},\\\"index\\\":0}],\\\"created\\\":1792119676,\\\"id\\\":\\\"chatcmpl-1\\\",\\\"model\\\":\\\"auto\\\",\\\"object\\\":\\\"chat.completion.chunk\\\"}\",\"statusCodeValue\":200,\"statusCode\":\"OK\"}\n\n",
        "data:{\"headers\":{},\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"!\\\"},\\\"finish_reason\\\":\\\"stop\\\",\\\"index\\\":0}],\\\"created\\\":1792119676,\\\"id\\\":\\\"chatcmpl-1\\\",\\\"model\\\":\\\"auto\\\",\\\"usage\\\":{\\\"prompt_tokens\\\":2,\\\"completion_tokens\\\":3,\\\"total_tokens\\\":5}}\",\"statusCodeValue\":200}\n\n",
        "data:{\"body\":\"[DONE]\",\"statusCodeValue\":200}\n\n"
    )
    .as_bytes();

    #[test]
    fn aggregates_real_gateway_stream_shape() {
        let mut aggregator = QoderChatAggregator::new();
        aggregator.push(SAMPLE).unwrap();
        let completion = aggregator.finish().unwrap();
        assert_eq!(completion["object"], "chat.completion");
        assert_eq!(completion["id"], "chatcmpl-1");
        assert_eq!(completion["model"], "auto");
        assert_eq!(completion["choices"][0]["message"]["content"], "Hello!");
        assert_eq!(completion["choices"][0]["finish_reason"], "stop");
        assert_eq!(completion["usage"]["total_tokens"], 5);
    }

    #[test]
    fn tool_call_deltas_merge_by_index() {
        let mut aggregator = QoderChatAggregator::new();
        let events = concat!(
            "data:{\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"tool_calls\\\":[{\\\"index\\\":0,\\\"id\\\":\\\"call_a\\\",\\\"type\\\":\\\"function\\\",\\\"function\\\":{\\\"name\\\":\\\"f\\\",\\\"arguments\\\":\\\"{\\\\\\\"x\\\\\\\":\\\"}}]}}]}\",\"statusCodeValue\":200}\n\n",
            "data:{\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"tool_calls\\\":[{\\\"index\\\":0,\\\"function\\\":{\\\"arguments\\\":\\\"1}\\\"}}]}}]}\",\"statusCodeValue\":200}\n\n"
        );
        aggregator.push(events.as_bytes()).unwrap();
        let completion = aggregator.finish().unwrap();
        let call = &completion["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(call["id"], "call_a");
        assert_eq!(call["function"]["name"], "f");
        assert_eq!(call["function"]["arguments"], r#"{"x":1}"#);
        assert_eq!(completion["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn status_envelope_becomes_error() {
        let mut aggregator = QoderChatAggregator::new();
        let events = "data:{\"statusCodeValue\":401,\"body\":\"denied\"}\n\n";
        aggregator.push(events.as_bytes()).unwrap();
        assert!(aggregator.finish().is_err());
    }

    #[test]
    fn partial_record_stays_pending() {
        let mut aggregator = QoderChatAggregator::new();
        aggregator.push(&SAMPLE[..SAMPLE.len() / 2]).unwrap();
        aggregator.push(&SAMPLE[SAMPLE.len() / 2..]).unwrap();
        assert!(aggregator.finish().is_ok());
    }

    /// 回归测试：连接被中途切断，最后一条记录没有空行终止符。
    /// 已聚合出的内容必须被保留（尽力解析残留），不再报 "ended mid-record"。
    #[test]
    fn truncated_tail_tolerated_when_content_exists() {
        let mut aggregator = QoderChatAggregator::new();
        // 前两条完整记录（内容 + usage），第三条残片：无结尾 \n\n。
        let complete = concat!(
            "data:{\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"Hello\\\",\\\"role\\\":\\\"assistant\\\"},\\\"index\\\":0}],\\\"id\\\":\\\"chatcmpl-1\\\",\\\"model\\\":\\\"auto\\\",\\\"object\\\":\\\"chat.completion.chunk\\\"}\",\"statusCodeValue\":200}\n\n",
            "data:{\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{},\\\"finish_reason\\\":\\\"stop\\\",\\\"index\\\":0}],\\\"usage\\\":{\\\"prompt_tokens\\\":2,\\\"completion_tokens\\\":3,\\\"total_tokens\\\":5}}\",\"statusCodeValue\":200}\n\n"
        );
        let residue = "event:finish\ndata:{\"firstTokenDuration\":357,\"totalDur";
        aggregator.push(complete.as_bytes()).unwrap();
        aggregator.push(residue.as_bytes()).unwrap();
        let completion = aggregator.finish().unwrap();
        assert_eq!(completion["choices"][0]["message"]["content"], "Hello");
        assert_eq!(completion["choices"][0]["finish_reason"], "stop");
        assert_eq!(completion["usage"]["total_tokens"], 5);
    }

    /// 完全没有内容时，截断仍然要报错（fail-closed：不能把失败伪装成空回复）。
    #[test]
    fn truncated_before_any_content_is_error() {
        let mut aggregator = QoderChatAggregator::new();
        // 残片解析不出任何有效事件（JSON 不完整）→ 无内容 → 必须报错。
        aggregator.push(b"data:{\"body\":\"{\\\"choi").unwrap();
        // finish 会尽力解析残留，解析失败且无内容 → Err。
        let result = aggregator.finish();
        assert!(result.is_err(), "got {result:?}");
    }
}
