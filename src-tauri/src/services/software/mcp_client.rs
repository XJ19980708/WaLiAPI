//! 最小 MCP 客户端：对接外部 MCP Server（stdio / Streamable HTTP）。
//!
//! 供 generic_mcp 聚合连接器使用——把社区现成 MCP Server 的工具代理进 WaLiAPI 的 /mcp。
//! 每次调用独立建立会话（stdio 冷启动 / HTTP 三次握手），换取无状态实现；外部 server
//! 通常自带缓存，代价可接受。工具发现与调用都有总超时保护。

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const PROTOCOL_VERSION: &str = "2024-11-05";
const CLIENT_INFO: &str = "waliapi-software-gateway";

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

fn json_request(id: u64, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params
    })
    .to_string()
}

fn notification(method: &str, params: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc":"2.0","method":method,"params":params}).to_string()
}

/// 从响应体提取 JSON-RPC 结果（兼容裸 JSON 与 SSE `data:` 行）。
fn parse_rpc_payload(body: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if v.get("result").is_some() || v.get("error").is_some() {
            return Some(v);
        }
    }
    for line in body.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if v.get("result").is_some() || v.get("error").is_some() {
                return Some(v);
            }
        }
    }
    None
}

fn rpc_error(resp: &serde_json::Value) -> Option<String> {
    resp.get("error").map(|e| {
        format!(
            "MCP server 返回错误 {}: {}",
            e.get("code")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            e.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
        )
    })
}

/// stdio 传输：spawn 子进程 → initialize → initialized → 请求 method，返回其 result。
pub async fn stdio_request(
    command: &str,
    args: &[String],
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // stderr 放行到继承（server 日志不参与协议），避免管道塞满阻塞
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("无法启动 MCP server `{command}`: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("MCP server stdin 不可用")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("MCP server stdout 不可用")?);

    let init = json_request(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": CLIENT_INFO, "version": "0.1"}
        }),
    );
    let write_err = |e: std::io::Error| format!("写入 MCP server 失败: {e}");
    stdin.write_all(init.as_bytes()).await.map_err(write_err)?;
    stdin.write_all(b"\n").await.map_err(write_err)?;
    stdin
        .write_all(notification("notifications/initialized", serde_json::json!({})).as_bytes())
        .await
        .map_err(write_err)?;
    stdin.write_all(b"\n").await.map_err(write_err)?;
    stdin
        .write_all(json_request(2, method, params).as_bytes())
        .await
        .map_err(write_err)?;
    stdin.write_all(b"\n").await.map_err(write_err)?;
    stdin.flush().await.map_err(write_err)?;
    drop(stdin);

    let deadline = tokio::time::Instant::now() + timeout;
    let mut line = String::new();
    loop {
        line.clear();
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "等待 MCP server `{command}` 响应超时（{}s）",
                timeout.as_secs()
            ));
        }
        let n = match tokio::time::timeout(remaining, stdout.read_line(&mut line)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(format!("读取 MCP server 输出失败: {e}")),
            Err(_) => {
                return Err(format!(
                    "等待 MCP server `{command}` 响应超时（{}s）",
                    timeout.as_secs()
                ))
            }
        };
        if n == 0 {
            return Err(format!(
                "MCP server `{command}` 提前退出（未返回 {method} 结果）"
            ));
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue; // 非 JSON 行（server 日志）跳过
        };
        if msg.get("id").and_then(serde_json::Value::as_u64) == Some(2) {
            if let Some(e) = rpc_error(&msg) {
                return Err(e);
            }
            return Ok(msg.get("result").cloned().unwrap_or(serde_json::json!({})));
        }
    }
}

/// Streamable HTTP 传输：initialize（捕获 session）→ initialized → 请求 method。
pub async fn http_request_rpc(
    url: &str,
    headers: &[(String, String)],
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;

    // 1. initialize
    let resp = post_json(
        &client,
        url,
        headers,
        json_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": CLIENT_INFO, "version": "0.1"}
            }),
        ),
    )
    .await?;
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let init_body = resp.text().await.unwrap_or_default();

    // 2. notifications/initialized
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(notification(
            "notifications/initialized",
            serde_json::json!({}),
        ));
    if let Some(s) = &session {
        req = req.header("mcp-session-id", s);
    }
    let _ = req.send().await;

    // 3. 实际请求
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(json_request(2, method, params));
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(s) = &session {
        req = req.header("mcp-session-id", s);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("MCP HTTP 请求失败: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() && body.is_empty() {
        return Err(format!("MCP HTTP 请求失败: HTTP {status}"));
    }
    let payload = parse_rpc_payload(&body)
        .or_else(|| parse_rpc_payload(&init_body))
        .ok_or_else(|| format!("MCP HTTP 响应无法解析为 JSON-RPC: {}", truncate_body(&body)))?;
    if let Some(e) = rpc_error(&payload) {
        return Err(e);
    }
    Ok(payload
        .get("result")
        .cloned()
        .unwrap_or(serde_json::json!({})))
}

fn truncate_body(body: &str) -> String {
    if body.len() > 200 {
        format!("{}…", &body[..200])
    } else {
        body.to_string()
    }
}

/// POST JSON-RPC body（带自定义头），返回原始响应。
async fn post_json(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: String,
) -> Result<reqwest::Response, String> {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req.send()
        .await
        .map_err(|e| format!("MCP HTTP 请求失败: {e}"))
}

/// 解析 tools/list result → 工具信息列表。
pub fn parse_tools_list(result: &serde_json::Value) -> Vec<McpToolInfo> {
    result
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(serde_json::Value::as_str)?;
                    Some(McpToolInfo {
                        name: name.to_string(),
                        description: t
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(name)
                            .to_string(),
                        input_schema: t.get("inputSchema").cloned().unwrap_or(serde_json::json!({
                            "type": "object", "properties": {}
                        })),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 tools/call result → MCP content 文本。
pub fn parse_call_result(result: &serde_json::Value) -> serde_json::Value {
    if result
        .get("isError")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let text = content_text(result);
        return serde_json::json!({"success": false, "error": text});
    }
    serde_json::json!({"success": true, "content": result.get("content").cloned().unwrap_or(serde_json::json!([])), "text": content_text(result)})
}

fn content_text(result: &serde_json::Value) -> String {
    result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|c| c.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_and_sse() {
        let plain = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;
        assert!(parse_rpc_payload(plain).is_some());
        let sse =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\n\n";
        assert!(parse_rpc_payload(sse).is_some());
        assert!(parse_rpc_payload("hello world").is_none());
    }

    #[test]
    fn parses_tools_list_shape() {
        let result = serde_json::json!({"tools": [
            {"name": "read_file", "description": "读文件", "inputSchema": {"type":"object"}},
            {"name": "no_desc"}
        ]});
        let tools = parse_tools_list(&result);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[1].description, "no_desc");
    }

    #[test]
    fn parses_call_result() {
        let ok = serde_json::json!({"content":[{"type":"text","text":"hi"}], "isError": false});
        assert_eq!(parse_call_result(&ok)["success"], serde_json::json!(true));
        assert_eq!(parse_call_result(&ok)["text"], serde_json::json!("hi"));
        let err = serde_json::json!({"content":[{"type":"text","text":"boom"}], "isError": true});
        assert_eq!(parse_call_result(&err)["success"], serde_json::json!(false));
    }

    /// stdio 端到端：spawn 一个最小 JSON-RPC echo server（python3），验证 tools/list。
    /// 环境无 python3 时跳过（返回 Ok）。
    #[tokio::test]
    async fn stdio_roundtrip_with_echo_server() {
        let which =
            executor::exec("which", &["python3".into()], None, Duration::from_secs(5)).await;
        if !which.success || which.stdout.trim().is_empty() {
            return; // 环境无 python3，跳过
        }
        let script = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if "id" not in msg:
        continue
    if msg.get("method") == "initialize":
        resp = {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"echo"}}}
    elif msg.get("method") == "tools/list":
        resp = {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ping","description":"echo test","inputSchema":{"type":"object"}}]}}
    else:
        continue
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()
"#;
        let result = stdio_request(
            "python3",
            &["-c".to_string(), script.to_string()],
            "tools/list",
            serde_json::json!({}),
            Duration::from_secs(15),
        )
        .await;
        let result = result.expect("stdio echo server 应返回 tools/list 结果");
        let tools = parse_tools_list(&result);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "ping");
    }

    use super::super::executor;
}
