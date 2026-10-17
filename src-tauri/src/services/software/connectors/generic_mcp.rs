//! MCP 聚合连接器（路径 E）：把外部 MCP Server 的工具代理进 WaLiAPI 的 /mcp。
//!
//! 社区已有大量现成 MCP Server（filesystem、playwright、各类软件官方 MCP），
//! 通过本连接器接入后其工具自动出现在 WaLiAPI 的 tools/list，一次投入持续受益。
//!
//! 配置示例：
//! ```json
//! {
//!   "servers": [
//!     {
//!       "name": "fs",
//!       "transport": "stdio",
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
//!       "risk": "read_only"
//!     },
//!     {
//!       "name": "deep",
//!       "transport": "http",
//!       "url": "http://127.0.0.1:9222/mcp",
//!       "headers": {},
//!       "risk": "read_only"
//!     }
//!   ],
//!   "timeout_secs": 60,
//!   "risk_overrides": { "fs__write_file": "high_write" }
//! }
//! ```
//! 工具短名 = `{server}__{远程工具名}`，完整 MCP 名形如
//! `software__mcp_3__fs__read_file`。

use super::super::connector::{RiskLevel, ToolManifest};
use super::super::mcp_client;
use super::super::model::SoftwareInstance;
use crate::services::software::connector::SoftwareConnector;
use async_trait::async_trait;
use std::time::Duration;

pub struct GenericMcpConnector;

fn servers_of(config: &serde_json::Value) -> Vec<serde_json::Value> {
    config
        .get("servers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn timeout_of(config: &serde_json::Value) -> Duration {
    Duration::from_secs(
        config
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .clamp(5, 600),
    )
}

fn server_risk(
    config: &serde_json::Value,
    server: &serde_json::Value,
    tool_short: &str,
) -> RiskLevel {
    if let Some(map) = config.get("risk_overrides").and_then(|v| v.as_object()) {
        if let Some(r) = map.get(tool_short).and_then(|v| v.as_str()) {
            return RiskLevel::parse(r);
        }
    }
    RiskLevel::parse(server.get("risk").and_then(|v| v.as_str()).unwrap_or(""))
}

fn transport_of(server: &serde_json::Value) -> Result<&'static str, String> {
    match server.get("transport").and_then(|v| v.as_str()) {
        Some("stdio") => Ok("stdio"),
        Some("http") | Some("streamable_http") => Ok("http"),
        other => Err(format!(
            "非法 transport `{}`（stdio / http）",
            other.unwrap_or("")
        )),
    }
}

/// 远程工具名清洗为短名安全字符（保留 `__` 分隔语义由外层处理）。
fn sanitize_tool_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if c.is_ascii_uppercase() {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "tool".to_string()
    } else {
        out
    }
}

async fn list_server_tools(
    config: &serde_json::Value,
    server: &serde_json::Value,
) -> Result<Vec<mcp_client::McpToolInfo>, String> {
    let transport = transport_of(server)?;
    let timeout = timeout_of(config);
    let result = match transport {
        "stdio" => {
            let command = server
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("stdio transport 缺少 command")?;
            let args: Vec<String> = server
                .get("args")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|a| a.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            mcp_client::stdio_request(command, &args, "tools/list", serde_json::json!({}), timeout)
                .await?
        }
        _ => {
            let url = server
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or("http transport 缺少 url")?;
            let headers: Vec<(String, String)> = server
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|map| {
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            mcp_client::http_request_rpc(
                url,
                &headers,
                "tools/list",
                serde_json::json!({}),
                timeout,
            )
            .await?
        }
    };
    Ok(mcp_client::parse_tools_list(&result))
}

async fn call_server_tool(
    config: &serde_json::Value,
    server: &serde_json::Value,
    remote_tool: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let transport = transport_of(server)?;
    let timeout = timeout_of(config);
    let params = serde_json::json!({"name": remote_tool, "arguments": args});
    let result = match transport {
        "stdio" => {
            let command = server
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("stdio transport 缺少 command")?;
            let srv_args: Vec<String> = server
                .get("args")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|a| a.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            mcp_client::stdio_request(command, &srv_args, "tools/call", params, timeout).await?
        }
        _ => {
            let url = server
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or("http transport 缺少 url")?;
            let headers: Vec<(String, String)> = server
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|map| {
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            mcp_client::http_request_rpc(url, &headers, "tools/call", params, timeout).await?
        }
    };
    let parsed = mcp_client::parse_call_result(&result);
    if parsed["success"] == serde_json::json!(false) {
        return Err(parsed["error"]
            .as_str()
            .unwrap_or("MCP 工具执行失败")
            .to_string());
    }
    Ok(parsed)
}

#[async_trait]
impl SoftwareConnector for GenericMcpConnector {
    fn id(&self) -> &'static str {
        "generic_mcp"
    }
    fn label(&self) -> &'static str {
        "MCP 聚合网关"
    }
    fn description(&self) -> &'static str {
        "接入社区现成 MCP Server（stdio / Streamable HTTP），其工具自动代理进 WaLiAPI 的 /mcp，工具名形如 fs__read_file"
    }
    fn platforms(&self) -> &'static str {
        "macOS / Windows / Linux"
    }
    fn sample_config(&self) -> serde_json::Value {
        serde_json::json!({
            "servers": [
                {
                    "name": "fs",
                    "transport": "stdio",
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                    "risk": "read_only"
                },
                {
                    "name": "deepwiki",
                    "transport": "http",
                    "url": "https://mcp.deepwiki.com/mcp",
                    "headers": {},
                    "risk": "read_only"
                }
            ],
            "timeout_secs": 60
        })
    }

    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String> {
        let servers = servers_of(config);
        if servers.is_empty() {
            return Err("config.servers 不能为空（至少接入一个 MCP Server）".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for (i, server) in servers.iter().enumerate() {
            let name = server
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or(format!("servers[{i}]: 缺少 name"))?;
            if !super::valid_tool_name(&sanitize_tool_name(name))
                || sanitize_tool_name(name) != name
            {
                return Err(format!(
                    "servers[{i}]: name `{name}` 须为小写 [a-z0-9_]（作为工具名前缀）"
                ));
            }
            if !seen.insert(name.to_string()) {
                return Err(format!("servers[{i}]: name `{name}` 重复"));
            }
            let transport = transport_of(server).map_err(|e| format!("servers[{i}]: {e}"))?;
            if transport == "stdio" {
                let command = server.get("command").and_then(|v| v.as_str()).unwrap_or("");
                if command.trim().is_empty() {
                    return Err(format!("servers[{i}]: stdio transport 缺少 command"));
                }
            } else {
                let url = server.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(format!("servers[{i}]: url 必须以 http:// 或 https:// 开头"));
                }
            }
            if let Some(args) = server.get("args") {
                if !args.is_array() || args.as_array().unwrap().iter().any(|a| !a.is_string()) {
                    return Err(format!("servers[{i}]: args 必须是字符串数组"));
                }
            }
        }
        if let Some(map) = config.get("risk_overrides") {
            if !map.is_object() || map.as_object().unwrap().values().any(|v| !v.is_string()) {
                return Err("risk_overrides 必须是字符串键值对象".to_string());
            }
        }
        Ok(())
    }

    async fn list_tools(&self, instance: &SoftwareInstance) -> Result<Vec<ToolManifest>, String> {
        let mut out = vec![];
        for server in servers_of(&instance.config) {
            let Some(name) = server.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            match list_server_tools(&instance.config, &server).await {
                Ok(tools) => {
                    for t in tools {
                        let short = sanitize_tool_name(&t.name);
                        let tool_short = format!("{name}__{short}");
                        out.push(ToolManifest {
                            name: tool_short.clone(),
                            description: format!(
                                "[{}] {}",
                                name,
                                if t.description.is_empty() {
                                    &t.name
                                } else {
                                    &t.description
                                }
                            ),
                            input_schema: t.input_schema.clone(),
                            risk: server_risk(&instance.config, &server, &tool_short),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!("[软件·MCP聚合] server `{name}` 工具发现失败: {e}");
                }
            }
        }
        Ok(out)
    }

    async fn call_tool(
        &self,
        instance: &SoftwareInstance,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let (server_name, remote_tool) = tool
            .split_once("__")
            .ok_or_else(|| format!("非法聚合工具名: {tool}"))?;
        let server = servers_of(&instance.config)
            .into_iter()
            .find(|s| s.get("name").and_then(|v| v.as_str()) == Some(server_name))
            .ok_or_else(|| format!("未接入的 MCP server: {server_name}"))?;
        call_server_tool(&instance.config, &server, remote_tool, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_mcp_names() {
        assert_eq!(sanitize_tool_name("read_file"), "read_file");
        assert_eq!(sanitize_tool_name("Read-File.7z"), "read_file_7z");
        assert_eq!(sanitize_tool_name("---"), "tool");
    }

    #[test]
    fn risk_overrides_apply() {
        let config = serde_json::json!({"risk_overrides": {"fs__write_file": "high_write"}});
        let server = serde_json::json!({"risk": "read_only"});
        assert_eq!(
            server_risk(&config, &server, "fs__write_file"),
            RiskLevel::HighWrite
        );
        assert_eq!(
            server_risk(&config, &server, "fs__read_file"),
            RiskLevel::ReadOnly
        );
    }

    #[test]
    fn validates_servers() {
        let conn = GenericMcpConnector;
        assert!(conn
            .validate_config(&serde_json::json!({"servers": []}))
            .is_err());
        assert!(conn
            .validate_config(&serde_json::json!({"servers": [{"name":"fs","transport":"stdio"}]}))
            .is_err());
        assert!(conn
            .validate_config(
                &serde_json::json!({"servers": [{"name":"fs","transport":"stdio","command":"npx"}]})
            )
            .is_ok());
        assert!(conn
            .validate_config(&serde_json::json!({"servers": [{"name":"d","transport":"http","url":"https://x/mcp"}]}))
            .is_ok());
        assert!(conn
            .validate_config(&serde_json::json!({"servers": [{"name":"Bad","transport":"stdio","command":"npx"}]}))
            .is_err());
    }
}
