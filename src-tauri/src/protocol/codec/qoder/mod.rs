//! Chat / Messages / Responses → Qoder `agent_chat_generation` 协议编解码器。
//!
//! Qoder 的 wire payload 是完全私有的 agent 格式（非 OpenAI 兼容）。已知约定
//! （逆向自官方客户端，参考 MIT 实现 pi-provider-qoder）：
//!
//! - 顶层 `system` 字段被服务端忽略，system prompt 必须注入为首条
//!   `role:system` 消息；
//! - `chat_context.extra.modelConfig.key` 承载上游模型 key（请求头
//!   `X-Model-Key` 与之一致，由 auth_provider 侧写入）；
//! - tool call id 必须匹配 `^[a-zA-Z0-9_-]{1,64}$`，非法 id 需确定性重写；
//! - 网关对**任何**请求（含 `stream:false`）都返回 SSE：
//!   `data:{"headers":...,"body":"<内层 JSON 字符串>","statusCodeValue":200}`，
//!   `body == "[DONE]"`（或裸 `[DONE]`）表示结束；
//! - 非流式路径必须把 SSE 聚合成 `chat.completion` 再解码（见 `aggregate`）。
//!
//! `Qoder` 是 upstream-only 协议：请求方向 Chat/Messages/Responses → Qoder，
//! 响应方向 Qoder → 各下游协议（经 Chat 中转，复用既有解码器）。

pub mod aggregate;

pub use aggregate::QoderChatAggregator;

use super::chat;
use super::direction::CodecDirection;
use super::error::{DecodeError, FeatureKind, PrepareError, UnsupportedFeatures};
use super::messages;
use super::ports::{DecodedResponse, NonStreamDecoder, StreamDecoder};
use super::report::{ConversionContext, Usage};
use super::types::{CodecId, Protocol};
use serde_json::{json, Value};

pub static CHAT_TO_QODER: ChatToQoder = ChatToQoder;
pub static MESSAGES_TO_QODER: MessagesToQoder = MessagesToQoder;
pub static RESPONSES_TO_QODER: ResponsesToQoder = ResponsesToQoder;

/// tool call id 线格式约束。
pub(crate) fn valid_tool_call_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 把非法 tool call id 确定性重写为 `qtc_<sha256 前 60 位>`。
pub(crate) fn rewrite_tool_call_id(id: &str) -> String {
    if valid_tool_call_id(id) {
        return id.to_owned();
    }
    let digest = sha2_digest(&format!("qoder-tool-call-id:v1\0{id}"));
    format!("qtc_{digest}")
}

fn sha2_digest(input: &str) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())[..60].to_owned()
}

/// 递归重写 payload 中所有 tool call id（assistant.tool_calls[].id 与
/// tool.tool_call_id），保证配对一致。
fn normalize_tool_call_ids(message: &mut Value) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    if let Some(calls) = object.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                let rewritten = rewrite_tool_call_id(id);
                if let Some(call_object) = call.as_object_mut() {
                    call_object.insert("id".to_owned(), json!(rewritten));
                }
            }
        }
    }
    if let Some(tool_call_id) = object.get("tool_call_id").and_then(Value::as_str) {
        let rewritten = rewrite_tool_call_id(tool_call_id);
        object.insert("tool_call_id".to_owned(), json!(rewritten));
    }
}

/// 构造 Qoder `agent_chat_generation` 请求体。
///
/// `chat` 是已转换的 OpenAI Chat 请求（codec 链路的上游公共表示）。
pub fn encode_chat_to_qoder(
    body: &Value,
    model: &str,
) -> Result<(Value, ConversionContext), UnsupportedFeatures> {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Err(UnsupportedFeatures::single(
            FeatureKind::UnknownRole,
            "/messages",
            "Chat request requires messages array",
        ));
    };
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);

    // system prompt：顶层 system 字段被服务端忽略，注入为首条 system 消息。
    let mut system_text = String::new();
    let mut normalized_messages: Vec<Value> = Vec::new();
    for message in messages {
        let mut message = message.clone();
        match message.get("role").and_then(Value::as_str) {
            Some("system") | Some("developer") => {
                if let Some(text) = message.get("content").and_then(Value::as_str) {
                    if !system_text.is_empty() {
                        system_text.push('\n');
                    }
                    system_text.push_str(text);
                }
                continue;
            }
            Some("assistant") | Some("tool") | Some("user") => {
                normalize_tool_call_ids(&mut message);
            }
            _ => {}
        }
        normalized_messages.push(message);
    }
    if !system_text.is_empty() {
        normalized_messages.insert(0, json!({"role": "system", "content": system_text}));
    }

    let record_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let last_user_text = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let tools = body.get("tools").cloned().unwrap_or_else(|| json!([]));

    // Qoder 服务端会忽略顶层 `system`，这里仍按官方客户端形态带上空值。
    let out = json!({
        "request_id": uuid::Uuid::new_v4().to_string(),
        "request_set_id": record_id,
        "chat_record_id": record_id,
        "session_id": session_id,
        "stream": true,
        "chat_task": "FREE_INPUT",
        "is_reply": true,
        "is_retry": false,
        "source": 1,
        "version": "3",
        "session_type": "qodercli",
        "agent_id": "agent_common",
        "task_id": "common",
        "code_language": "",
        "chat_prompt": "",
        "image_urls": Value::Null,
        "aliyun_user_type": "",
        "system": "",
        "messages": normalized_messages,
        "tools": tools,
        "parameters": {},
        "chat_context": {
            "chatPrompt": "",
            "imageUrls": Value::Null,
            "extra": {
                "context": [],
                "modelConfig": {
                    "key": model,
                    "is_reasoning": false,
                },
                "originalContent": last_user_text,
            },
            "features": [],
            "text": last_user_text,
        },
        "business": {
            "product": "cli",
            "version": "1.0.0",
            "type": "agent",
            "stage": "start",
            "id": uuid::Uuid::new_v4().to_string(),
            "name": last_user_text.chars().take(30).collect::<String>(),
            "begin_at": chrono::Utc::now().timestamp_millis(),
        },
    });

    let request_id = body
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
    let mut context = ConversionContext::new(request_id, model, stream);
    context.normalized.push("/system".to_owned());
    Ok((out, context))
}

/// 从 Qoder 网关信封中提取内层 OpenAI 风格 chunk / 完成体。
///
/// 信封形态：`{"statusCodeValue": 200, "body": "<内层 JSON 字符串>"}`，
/// `body == "[DONE]"`（或裸 `[DONE]`）表示流结束。
pub(crate) fn unwrap_envelope(event: &Value) -> Result<Option<Value>, DecodeError> {
    if event.get("statusCodeValue").is_some() || event.get("body").is_some() {
        let status = event
            .get("statusCodeValue")
            .and_then(Value::as_u64)
            .unwrap_or(200);
        if status >= 400 {
            return Err(DecodeError::new(
                "/statusCodeValue",
                format!("Qoder gateway envelope status {status}"),
            ));
        }
        let body = event.get("body").cloned().unwrap_or(Value::Null);
        return match body {
            Value::String(text) => {
                if text == "[DONE]" || text.is_empty() {
                    Ok(None)
                } else {
                    serde_json::from_str(&text).map(Some).map_err(|error| {
                        DecodeError::new("/", format!("Qoder envelope body JSON invalid: {error}"))
                    })
                }
            }
            Value::Null => Ok(None),
            other => Ok(Some(other)),
        };
    }
    // 裸 [DONE] 字符串事件（个别网关形态）。
    if event.is_string() && event.as_str() == Some("[DONE]") {
        return Ok(None);
    }
    Ok(Some(event.clone()))
}

// ---------------------------------------------------------------------------
// Directions
// ---------------------------------------------------------------------------

pub struct ChatToQoder;
impl CodecDirection for ChatToQoder {
    fn id(&self) -> CodecId {
        CodecId::ChatToQoderV1
    }
    fn downstream(&self) -> Protocol {
        Protocol::Chat
    }
    fn upstream(&self) -> Protocol {
        Protocol::Qoder
    }
    fn encode_request(
        &self,
        request: &Value,
        model: &str,
    ) -> Result<(Value, ConversionContext), PrepareError> {
        encode_chat_to_qoder(request, model)
    }
    fn new_response_decoder(
        &self,
        context: &ConversionContext,
    ) -> Box<dyn NonStreamDecoder + Send + Sync> {
        Box::new(QoderThenChatTo {
            inner: context.clone(),
            downstream: Protocol::Chat,
        })
    }
    fn new_stream_response_decoder(
        &self,
        context: &ConversionContext,
    ) -> Box<dyn StreamDecoder + Send + Sync> {
        // 下游 Chat：内层解信封后本就是 Chat chunk，直通即可（含 [DONE] 终止）。
        Box::new(QoderStreamDecoder::new(
            context.clone(),
            Box::new(ChatPassthroughStreamDecoder::default()),
        ))
    }
}

pub struct MessagesToQoder;
impl CodecDirection for MessagesToQoder {
    fn id(&self) -> CodecId {
        CodecId::MessagesToQoderV1
    }
    fn downstream(&self) -> Protocol {
        Protocol::Messages
    }
    fn upstream(&self) -> Protocol {
        Protocol::Qoder
    }
    fn encode_request(
        &self,
        request: &Value,
        model: &str,
    ) -> Result<(Value, ConversionContext), PrepareError> {
        let (chat_body, first_context) = messages::encode_messages_to_chat(request, model)?;
        let (encoded, mut context) = encode_chat_to_qoder(&chat_body, model)?;
        context.normalized.extend(first_context.normalized);
        Ok((encoded, context))
    }
    fn new_response_decoder(
        &self,
        context: &ConversionContext,
    ) -> Box<dyn NonStreamDecoder + Send + Sync> {
        Box::new(QoderThenChatTo {
            inner: context.clone(),
            downstream: Protocol::Messages,
        })
    }
    fn new_stream_response_decoder(
        &self,
        context: &ConversionContext,
    ) -> Box<dyn StreamDecoder + Send + Sync> {
        Box::new(QoderStreamDecoder::new(
            context.clone(),
            chat::ChatStreamDecoder::boxed(context),
        ))
    }
}

pub struct ResponsesToQoder;
impl CodecDirection for ResponsesToQoder {
    fn id(&self) -> CodecId {
        CodecId::ResponsesToQoderV1
    }
    fn downstream(&self) -> Protocol {
        Protocol::Responses
    }
    fn upstream(&self) -> Protocol {
        Protocol::Qoder
    }
    fn encode_request(
        &self,
        request: &Value,
        model: &str,
    ) -> Result<(Value, ConversionContext), PrepareError> {
        // Responses → Chat（复用 registry 的单跳 wrapper 逻辑）→ Qoder。
        let mut chat_body = crate::protocol::responses_to_openai(request)?;
        chat_body
            .as_object_mut()
            .ok_or_else(|| {
                UnsupportedFeatures::single(
                    FeatureKind::UnsupportedField,
                    "/",
                    "Responses to Chat encoder produced a non-object request",
                )
            })?
            .insert("model".to_owned(), json!(model));
        let (encoded, context) = encode_chat_to_qoder(&chat_body, model)?;
        Ok((encoded, context))
    }
    fn new_response_decoder(
        &self,
        context: &ConversionContext,
    ) -> Box<dyn NonStreamDecoder + Send + Sync> {
        Box::new(QoderThenChatTo {
            inner: context.clone(),
            downstream: Protocol::Responses,
        })
    }
    fn new_stream_response_decoder(
        &self,
        context: &ConversionContext,
    ) -> Box<dyn StreamDecoder + Send + Sync> {
        // 下游 Responses：内层 Chat SSE → Responses SSE（与 Gemini 管道一致）。
        Box::new(QoderStreamDecoder::new(
            context.clone(),
            Box::new(super::responses_codec::ChatToResponsesStreamDecoder::new(
                context,
            )),
        ))
    }
}

// ---------------------------------------------------------------------------
// Response decoders
// ---------------------------------------------------------------------------

/// Qoder 完成体 → 下游协议。
///
/// 非流式路径的输入已是 [`QoderChatAggregator`] 聚合出的完整
/// `chat.completion`（信封与 SSE 早在 executor 层解开），这里只做协议
/// 转换；若上游仍直接给出信封形态（如部分网关部署），先解信封兜底。
struct QoderThenChatTo {
    inner: ConversionContext,
    downstream: Protocol,
}

impl NonStreamDecoder for QoderThenChatTo {
    fn decode(&self, body: &Value) -> Result<DecodedResponse, DecodeError> {
        // 兼容两种输入形态：已聚合的 chat.completion（object 且带 choices）
        // 或原始信封。
        let inner = if body.get("choices").is_some() {
            body.clone()
        } else {
            unwrap_envelope(body)?
                .ok_or_else(|| DecodeError::new("/", "Qoder response body was empty"))?
        };
        // `llm_model_result` 附加字段直接忽略。
        let inner = inner.get("llm_model_result").cloned().unwrap_or(inner);
        match self.downstream {
            Protocol::Chat => Ok(DecodedResponse {
                usage: super::identity::parse_usage(Protocol::Chat, &inner),
                body: inner,
            }),
            Protocol::Messages => {
                let decoded = chat::decode_chat_response_to_messages(&inner, &self.inner)
                    .map_err(DecodeError::from_unsupported)?;
                Ok(DecodedResponse {
                    body: decoded,
                    usage: super::identity::parse_usage(Protocol::Chat, &inner),
                })
            }
            Protocol::Responses => {
                let decoded =
                    crate::protocol::openai_to_responses(&inner, &self.inner.upstream_model);
                Ok(DecodedResponse {
                    body: decoded,
                    usage: super::identity::parse_usage(Protocol::Chat, &inner),
                })
            }
            _ => Err(DecodeError::new("/", "unsupported downstream protocol")),
        }
    }
}

/// Qoder SSE → 内层 Chat SSE → 下游 SSE 管道解码器。
struct QoderStreamDecoder {
    inner: Box<dyn StreamDecoder + Send + Sync>,
    pending: Vec<u8>,
    ended: bool,
    /// 是否已向下游产出过事件（截断容错判据，与聚合器 saw_event 对齐）。
    saw_output: bool,
}

impl QoderStreamDecoder {
    fn new(_context: ConversionContext, inner: Box<dyn StreamDecoder + Send + Sync>) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            ended: false,
            saw_output: false,
        }
    }

    /// 解析并消费一条完整 SSE 记录，返回转发给下游的事件。
    /// `[DONE]` / 信封 `[DONE]` 会终结本流（`ended = true`）。
    fn consume_record_bytes(&mut self, record: &[u8]) -> Result<Vec<String>, DecodeError> {
        let mut out = Vec::new();
        let payload =
            super::sse::parse_data_payload(record).map_err(DecodeError::from_unsupported)?;
        if payload.is_empty() {
            return Ok(out);
        }
        if payload == "[DONE]" {
            // 网关层 [DONE]：转发内层 [DONE] 语义（由 inner.finish 补齐）。
            out.extend(self.inner.finish()?);
            self.ended = true;
            return Ok(out);
        }
        let event: Value = serde_json::from_str(&payload).map_err(|error| {
            DecodeError::new("/", format!("Qoder SSE JSON was invalid: {error}"))
        })?;
        let Some(inner) = unwrap_envelope(&event)? else {
            // 信封 [DONE] 或空 body。
            out.extend(self.inner.finish()?);
            self.ended = true;
            return Ok(out);
        };
        // 网关遥测事件（`event:finish` + firstTokenDuration 等）既非信封也非
        // chat chunk，直通会成为下游垃圾帧；没有 choices/usage/id 的一律跳过。
        if inner.get("choices").is_none()
            && inner.get("usage").is_none()
            && inner.get("id").is_none()
        {
            return Ok(out);
        }
        // 内层事件必须重新组帧为 SSE 记录（`data: {json}\n\n`）再交给下游
        // 解码器：内层解码器（ChatSseState / Responses 解码器 / Chat 直通）
        // 的输入契约都是 SSE 记录，裸 JSON 没有终止符与 data: 前缀，会被
        // 当作空载荷静默丢弃（回归：流式路径全部报 "no message_start"）。
        let inner_bytes = format!("data: {inner}\n\n").into_bytes();
        let events = self.inner.feed(&inner_bytes)?;
        if !events.is_empty() {
            self.saw_output = true;
        }
        out.extend(events);
        Ok(out)
    }
}

impl StreamDecoder for QoderStreamDecoder {
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, DecodeError> {
        self.pending.extend_from_slice(bytes);
        if self.ended {
            self.pending.clear();
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        loop {
            // Qoder 网关的 SSE 记录同样以空行分隔。
            let Some(length) = super::sse::record_end(&self.pending) else {
                break;
            };
            let record: Vec<u8> = self.pending.drain(..length).collect();
            out.extend(self.consume_record_bytes(&record)?);
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<String>, DecodeError> {
        if self.ended {
            return Ok(Vec::new());
        }
        // 流被提前切断（网关/中间代理在记录中途关闭）：把残留字节尽力解析。
        // 有内层事件产出则容忍尾部残片；完全没有内容才向上报协议错误。
        if !self.pending.is_empty() {
            let residue = std::mem::take(&mut self.pending);
            if let Ok(events) = self.consume_record_bytes(&residue) {
                if !events.is_empty() {
                    self.ended = true;
                    return Ok(events);
                }
            }
            if self.saw_output {
                // 已向下游产出过内容：尾部残片（多为遥测/截断）容忍，
                // 交给内层解码器正常收尾（合成终止事件）。
                return out_finish(&mut self.inner);
            }
            return Err(DecodeError::new("/", "Qoder SSE ended mid-record"));
        }
        out_finish(&mut self.inner)
    }

    fn usage(&self) -> Option<super::report::Usage> {
        self.inner.usage()
    }

    fn saw_terminal(&self) -> bool {
        self.ended && self.inner.saw_terminal()
    }
}

/// Qoder 内层 Chat chunk → 下游 Chat SSE 直通解码器。
///
/// 解信封后的内层事件本身就是 OpenAI Chat chunk，下游为 Chat 协议时无需
/// 协议转换，重组帧直通；usage 从 chunk 提取用于记账，网关终止标记
/// （信封 `[DONE]` / 裸 `[DONE]` / EOF 收尾）合成下游 `data: [DONE]`。
#[derive(Default)]
struct ChatPassthroughStreamDecoder {
    usage: Usage,
    saw_chunk: bool,
    done_emitted: bool,
}

impl ChatPassthroughStreamDecoder {
    fn emit_done(&mut self, out: &mut Vec<String>) {
        if self.done_emitted {
            return;
        }
        self.done_emitted = true;
        out.push("data: [DONE]\n\n".to_owned());
    }

    fn merge_usage(&mut self, u: &Value) {
        let prompt = u.get("prompt_tokens").and_then(Value::as_u64);
        let completion = u.get("completion_tokens").and_then(Value::as_u64);
        if let Some(p) = prompt {
            self.usage.input_tokens = p;
        }
        if let Some(c) = completion {
            self.usage.output_tokens = c;
        }
        if prompt.is_none()
            && completion.is_none()
            && self.usage.input_tokens == 0
            && self.usage.output_tokens == 0
        {
            self.usage.usage_unknown = true;
        }
    }
}

impl StreamDecoder for ChatPassthroughStreamDecoder {
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, DecodeError> {
        // 输入是 QoderStreamDecoder 重组帧后的单条 Chat SSE 记录。
        let mut out = Vec::new();
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| DecodeError::new("/", "Qoder inner chunk was not valid UTF-8"))?;
        let Some(payload) = text
            .strip_prefix("data: ")
            .and_then(|rest| rest.strip_suffix("\n\n"))
        else {
            return Ok(out);
        };
        if payload.is_empty() {
            return Ok(out);
        }
        if payload == "[DONE]" {
            self.emit_done(&mut out);
            return Ok(out);
        }
        let event: Value = serde_json::from_str(payload).map_err(|error| {
            DecodeError::new("/", format!("Qoder inner chunk JSON was invalid: {error}"))
        })?;
        if let Some(u) = event.get("usage").filter(|u| u.is_object()) {
            self.merge_usage(u);
        }
        if event.get("choices").is_some() {
            self.saw_chunk = true;
        }
        out.push(format!("data: {payload}\n\n"));
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<String>, DecodeError> {
        let mut out = Vec::new();
        if self.saw_chunk || self.done_emitted {
            self.emit_done(&mut out);
            return Ok(out);
        }
        Err(DecodeError::new(
            "/",
            "Qoder upstream stream ended before any chat chunk",
        ))
    }

    fn usage(&self) -> Option<Usage> {
        Some(self.usage)
    }

    fn saw_terminal(&self) -> bool {
        self.done_emitted
    }
}

fn out_finish(
    inner: &mut Box<dyn StreamDecoder + Send + Sync>,
) -> Result<Vec<String>, DecodeError> {
    inner.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_tool_call_ids_pass_through() {
        assert!(valid_tool_call_id("call_abc-123"));
        assert!(!valid_tool_call_id("call|bad"));
        assert!(!valid_tool_call_id(""));
        assert!(!valid_tool_call_id(&"a".repeat(65)));
        assert!(valid_tool_call_id(&"a".repeat(64)));
    }

    #[test]
    fn rewrite_is_deterministic_and_wire_valid() {
        let bad = "call_A|fc_B";
        let first = rewrite_tool_call_id(bad);
        let second = rewrite_tool_call_id(bad);
        assert_eq!(first, second);
        assert!(valid_tool_call_id(&first));
        assert!(first.starts_with("qtc_"));
        // 合法 id 原样保留
        assert_eq!(rewrite_tool_call_id("call_ok"), "call_ok");
        // 不同非法 id 映射不同结果
        assert_ne!(rewrite_tool_call_id("a|b"), rewrite_tool_call_id("a|c"));
    }

    #[test]
    fn chat_to_qoder_injects_system_message_and_ignores_top_level_system() {
        let request = json!({
            "model": "Lite",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hi"}
            ],
            "stream": false
        });
        let (encoded, _context) = encode_chat_to_qoder(&request, "Lite").unwrap();
        assert_eq!(encoded["stream"], true, "Qoder 网关只接受流式");
        assert_eq!(encoded["chat_task"], "FREE_INPUT");
        assert_eq!(encoded["agent_id"], "agent_common");
        let messages = encoded["messages"].as_array().unwrap();
        // system 注入为首条
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be nice");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(
            encoded["chat_context"]["extra"]["modelConfig"]["key"],
            "Lite"
        );
        assert_eq!(encoded["business"]["product"], "cli");
    }

    #[test]
    fn chat_to_qoder_rewrites_bad_tool_call_ids() {
        let request = json!({
            "model": "Lite",
            "messages": [
                {"role": "assistant", "tool_calls": [{"id": "call_A|fc_B", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "call_A|fc_B", "content": "done"}
            ]
        });
        let (encoded, _) = encode_chat_to_qoder(&request, "Lite").unwrap();
        let messages = encoded["messages"].as_array().unwrap();
        let rewritten = messages[0]["tool_calls"][0]["id"].as_str().unwrap();
        assert!(valid_tool_call_id(rewritten), "got {rewritten}");
        assert_eq!(messages[1]["tool_call_id"], json!(rewritten));
    }

    #[test]
    fn envelope_unwrap_handles_string_body_and_done() {
        let event = json!({"statusCodeValue": 200, "body": "{\"id\":\"x\"}"});
        let inner = unwrap_envelope(&event).unwrap().unwrap();
        assert_eq!(inner["id"], "x");

        // [DONE] 信封
        let done = json!({"statusCodeValue": 200, "body": "[DONE]"});
        assert!(unwrap_envelope(&done).unwrap().is_none());

        // 裸 [DONE]
        assert!(unwrap_envelope(&json!("[DONE]")).unwrap().is_none());

        // 非 2xx 信封 → 错误
        assert!(unwrap_envelope(&json!({"statusCodeValue": 401, "body": "denied"})).is_err());
    }

    #[test]
    fn messages_to_qoder_composes_through_chat() {
        let request = json!({
            "model": "Lite",
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "hi"}
            ],
            "max_tokens": 100
        });
        let (encoded, context) = MessagesToQoder.encode_request(&request, "Lite").unwrap();
        assert_eq!(encoded["agent_id"], "agent_common");
        assert_eq!(context.upstream_model, "Lite");
    }

    // -----------------------------------------------------------------------
    // 流式解码回归：QoderStreamDecoder 必须把解信封后的内层 chunk 重新组帧
    // 为 SSE 记录（`data: {json}\n\n`）再喂给内层解码器。裸 JSON 会被内层
    // SSE 解析当作空载荷静默丢弃，最终报 "no message_start emitted"。
    // -----------------------------------------------------------------------

    /// 真实网关信封流：chunk → finish+usage → 信封 [DONE]。
    const GATEWAY_STREAM: &str = concat!(
        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"role\\\":\\\"assistant\\\",\\\"content\\\":\\\"Hello\\\"},\\\"index\\\":0}],\\\"id\\\":\\\"chatcmpl-1\\\",\\\"model\\\":\\\"auto\\\",\\\"object\\\":\\\"chat.completion.chunk\\\"}\",\"statusCodeValue\":200}\n\n",
        "data:{\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{},\\\"finish_reason\\\":\\\"stop\\\",\\\"index\\\":0}],\\\"id\\\":\\\"chatcmpl-1\\\",\\\"model\\\":\\\"auto\\\",\\\"usage\\\":{\\\"prompt_tokens\\\":2,\\\"completion_tokens\\\":3,\\\"total_tokens\\\":5}}\",\"statusCodeValue\":200}\n\n",
        "data:{\"body\":\"[DONE]\",\"statusCodeValue\":200}\n\n"
    );

    /// 前两条记录（无终止标记），用于截断容错测试。
    fn gateway_stream_without_done() -> String {
        GATEWAY_STREAM
            .split_once("data:{\"body\":\"[DONE]\"")
            .expect("stream contains done record")
            .0
            .to_owned()
    }

    #[test]
    fn stream_messages_downstream_emits_full_message_lifecycle() {
        let context = ConversionContext::new("req-1", "Lite", true);
        let mut decoder = MESSAGES_TO_QODER.new_stream_response_decoder(&context);
        let mut events = decoder.feed(GATEWAY_STREAM.as_bytes()).unwrap();
        events.extend(decoder.finish().unwrap());
        let text = events.join("");
        assert!(text.contains("\"message_start\""), "got {text}");
        assert!(text.contains("\"text_delta\""), "got {text}");
        assert!(text.contains("Hello"), "got {text}");
        assert!(text.contains("\"message_stop\""), "got {text}");
    }

    #[test]
    fn stream_chat_downstream_passthroughs_chunks_and_done() {
        let context = ConversionContext::new("req-1", "Lite", true);
        let mut decoder = CHAT_TO_QODER.new_stream_response_decoder(&context);
        let events = decoder.feed(GATEWAY_STREAM.as_bytes()).unwrap();
        let text = events.join("");
        assert!(text.contains("chat.completion.chunk"), "got {text}");
        assert!(text.contains("\"choices\""), "got {text}");
        assert!(text.contains("data: [DONE]\n\n"), "got {text}");
        assert!(decoder.saw_terminal());
        // EOF 后 finish 不得重复输出终止标记。
        assert!(decoder.finish().unwrap().is_empty());
    }

    #[test]
    fn stream_responses_downstream_completes_response_lifecycle() {
        let context = ConversionContext::new("req-1", "Lite", true);
        let mut decoder = RESPONSES_TO_QODER.new_stream_response_decoder(&context);
        let mut events = decoder.feed(GATEWAY_STREAM.as_bytes()).unwrap();
        events.extend(decoder.finish().unwrap());
        let text = events.join("");
        assert!(text.contains("response.created"), "got {text}");
        assert!(text.contains("response.completed"), "got {text}");
        assert!(text.contains("data: [DONE]"), "got {text}");
    }

    #[test]
    fn stream_decoder_skips_gateway_telemetry_events() {
        let context = ConversionContext::new("req-1", "Lite", true);
        let mut decoder = CHAT_TO_QODER.new_stream_response_decoder(&context);
        let stream = format!(
            "{}event:finish\ndata:{{\"firstTokenDuration\":357,\"totalDuration\":900}}\n\n{}",
            gateway_stream_without_done(),
            "data:{\"body\":\"[DONE]\",\"statusCodeValue\":200}\n\n"
        );
        let events = decoder.feed(stream.as_bytes()).unwrap();
        let text = events.join("");
        assert!(!text.contains("firstTokenDuration"), "got {text}");
        assert!(text.contains("data: [DONE]\n\n"), "got {text}");
    }

    #[test]
    fn stream_decoder_tolerates_truncated_tail_after_content() {
        let context = ConversionContext::new("req-1", "Lite", true);
        let mut decoder = MESSAGES_TO_QODER.new_stream_response_decoder(&context);
        let mut events = decoder
            .feed(gateway_stream_without_done().as_bytes())
            .unwrap();
        // 尾部残片：网关在最后一条遥测记录未带终止符时关闭连接。
        events.extend(
            decoder
                .feed(b"event:finish\ndata:{\"firstTokenDuration\":357,\"totalDur")
                .unwrap(),
        );
        events.extend(decoder.finish().unwrap());
        let text = events.join("");
        assert!(text.contains("\"message_stop\""), "got {text}");
        assert!(!text.contains("firstTokenDuration"), "got {text}");
    }

    #[test]
    fn stream_decoder_fails_closed_when_terminated_without_content() {
        // 网关只回了 [DONE] 信封、没有任何内容帧：必须报错（fail-closed），
        // 不能把失败伪装成空回复。
        let context = ConversionContext::new("req-1", "Lite", true);
        let mut decoder = MESSAGES_TO_QODER.new_stream_response_decoder(&context);
        let result = decoder.feed(b"data:{\"body\":\"[DONE]\",\"statusCodeValue\":200}\n\n");
        assert!(result.is_err());
    }
}
