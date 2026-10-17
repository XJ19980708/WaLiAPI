//! 内置软件连接器。
//!
//! - `generic_cli`：通用命令行连接器（覆盖路径 A 原生 CLI / 路径 B 脚本桥——
//!   `program` 填 `osascript` / `powershell` 即是脚本桥），macOS + Windows
//! - `generic_http`：通用本地 HTTP 连接器（覆盖路径 A 的本地 API 软件）
//! - `wechat`：微信桌面端实验性连接器（macOS 脚本桥 + UI 自动化，P2）
//! - `generic_mcp`：MCP 聚合网关（路径 E，代理社区现成 MCP Server，P3）

pub mod generic_cli;
pub mod generic_http;
pub mod generic_mcp;
pub mod wechat;

use super::connector::SoftwareConnector;
use std::sync::OnceLock;

static REGISTRY: OnceLock<Vec<Box<dyn SoftwareConnector>>> = OnceLock::new();

/// 全部内置连接器（连接器市场数据源）。
pub fn all() -> &'static [Box<dyn SoftwareConnector>] {
    REGISTRY.get_or_init(|| {
        vec![
            Box::new(generic_cli::GenericCliConnector),
            Box::new(generic_http::GenericHttpConnector),
            Box::new(generic_mcp::GenericMcpConnector),
            Box::new(wechat::WeChatConnector),
        ]
    })
}

/// 按 id 取连接器。
pub fn get(connector_id: &str) -> Option<&'static dyn SoftwareConnector> {
    all().iter().find(|c| c.id() == connector_id).map(|b| &**b)
}

/// 工具短名校验：MCP 命名空间内必须安全可拼（`software__{slug}__{tool}`）。
pub fn valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.len() <= 50
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 由 params 声明构建 JSON Schema（inputSchema）。
/// param: `{ "name": "...", "description": "...", "required": bool }`。
pub fn build_input_schema(params: &serde_json::Value) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = vec![];
    if let Some(list) = params.as_array() {
        for p in list {
            let Some(name) = p.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            if !valid_tool_name(name) {
                continue;
            }
            let description = p
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or(name);
            properties.insert(
                name.to_string(),
                serde_json::json!({ "type": "string", "description": description }),
            );
            if p.get("required").and_then(|v| v.as_bool()).unwrap_or(false) {
                required.push(serde_json::json!(name));
            }
        }
    }
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = serde_json::json!(required);
    }
    schema
}

/// 从 params 声明中提取参数名列表。
pub fn param_names(params: &serde_json::Value) -> Vec<String> {
    params
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| p.get("name").and_then(|v| v.as_str()))
                .filter(|n| valid_tool_name(n))
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_rules() {
        assert!(valid_tool_name("list_chats"));
        assert!(valid_tool_name("a1_b"));
        assert!(!valid_tool_name("ListChats"));
        assert!(!valid_tool_name("1abc"));
        assert!(!valid_tool_name("a b"));
        assert!(!valid_tool_name("bad;name"));
    }

    #[test]
    fn schema_builder() {
        let params = serde_json::json!([
            {"name":"query","description":"关键词","required":true},
            {"name":"limit","description":"条数"}
        ]);
        let schema = build_input_schema(&params);
        assert_eq!(schema["properties"]["query"]["type"], "string");
        assert_eq!(schema["required"], serde_json::json!(["query"]));
    }

    #[test]
    fn registry_contains_builtin_connectors() {
        let ids: Vec<&str> = all().iter().map(|c| c.id()).collect();
        assert!(ids.contains(&"generic_cli"));
        assert!(ids.contains(&"generic_http"));
        assert!(get("generic_cli").is_some());
        assert!(get("nope").is_none());
    }
}
