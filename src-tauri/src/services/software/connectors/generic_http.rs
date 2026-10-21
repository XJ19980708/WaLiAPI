//! 通用本地 HTTP 连接器。
//!
//! 对接自带本地 API 的软件（Ollama、qBittorrent WebUI、Jupyter、自研服务等）：
//! 每个端点登记为一个 MCP 工具，路径/查询/请求体均支持 `{{param}}` 模板。
//!
//! 配置示例：
//! ```json
//! {
//!   "base_url": "http://127.0.0.1:11434",
//!   "headers": { "Authorization": "Bearer sk-xxx" },
//!   "endpoints": [
//!     {
//!       "tool": "list_models",
//!       "description": "列出本地模型",
//!       "method": "GET",
//!       "path": "/v1/models",
//!       "risk": "read_only",
//!       "timeout_secs": 20
//!     }
//!   ]
//! }
//! ```

use super::super::connector::{RiskLevel, ToolManifest};
use super::super::executor;
use super::super::model::SoftwareInstance;
use super::{build_input_schema, template_placeholders, valid_tool_name};
use crate::services::software::connector::SoftwareConnector;
use async_trait::async_trait;
use std::time::Duration;

pub struct GenericHttpConnector;

const ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

fn endpoints_of(config: &serde_json::Value) -> Vec<serde_json::Value> {
    config
        .get("endpoints")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn headers_of(config: &serde_json::Value) -> Vec<(String, String)> {
    config
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn manifest_for(entry: &serde_json::Value) -> Result<ToolManifest, String> {
    let name = entry
        .get("tool")
        .and_then(|v| v.as_str())
        .ok_or("缺少 tool 字段")?;
    if !valid_tool_name(name) {
        return Err(format!(
            "非法 tool 名 `{name}`（须为小写字母开头的 [a-z0-9_]）"
        ));
    }
    let method = entry
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();
    if !ALLOWED_METHODS.contains(&method.as_str()) {
        return Err(format!("非法 HTTP 方法 `{method}`"));
    }
    let description = entry
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or(name);
    let risk = RiskLevel::parse(entry.get("risk").and_then(|v| v.as_str()).unwrap_or(""));
    let params = entry
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    let mut input_schema = build_input_schema(&params);
    // 路径模板占位符（如 /tasks/{{taskId}}）自动登记为必填参数：
    // 调用方按 schema 传参后，executor 才能完成 URL 模板渲染
    let path_tpl = entry.get("path").and_then(|v| v.as_str()).unwrap_or("");
    for ph in template_placeholders(path_tpl) {
        let props = input_schema
            .get_mut("properties")
            .and_then(|v| v.as_object_mut())
            .expect("build_input_schema 必产出 properties");
        props.entry(ph.clone()).or_insert_with(|| {
            serde_json::json!({
                "type": "string",
                "description": format!("路径参数 {ph}（必填）"),
            })
        });
        let required = input_schema
            .as_object_mut()
            .unwrap()
            .entry("required")
            .or_insert_with(|| serde_json::json!([]));
        if let Some(arr) = required.as_array_mut() {
            if !arr.iter().any(|x| x.as_str() == Some(ph.as_str())) {
                arr.push(serde_json::json!(ph));
            }
        }
    }
    Ok(ToolManifest {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        risk,
    })
}

#[async_trait]
impl SoftwareConnector for GenericHttpConnector {
    fn id(&self) -> &'static str {
        "generic_http"
    }
    fn label(&self) -> &'static str {
        "通用本地 HTTP"
    }
    fn description(&self) -> &'static str {
        "对接自带本地 HTTP API 的软件（Ollama、下载器、自研服务等），每个端点登记为一个 MCP 工具"
    }
    fn platforms(&self) -> &'static str {
        "macOS / Windows / Linux"
    }
    fn sample_config(&self) -> serde_json::Value {
        serde_json::json!({
            "base_url": "http://127.0.0.1:11434",
            "headers": {},
            "endpoints": [
                {
                    "tool": "list_models",
                    "description": "列出本地已安装模型",
                    "method": "GET",
                    "path": "/v1/models",
                    "risk": "read_only",
                    "timeout_secs": 20
                },
                {
                    "tool": "get_torrents",
                    "description": "查询下载任务列表",
                    "method": "GET",
                    "path": "/api/v2/torrents/info",
                    "params": [{"name": "filter", "description": "过滤条件，如 completed"}],
                    "risk": "read_only",
                    "timeout_secs": 20
                }
            ]
        })
    }

    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String> {
        let base_url = config
            .get("base_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err("base_url 必须以 http:// 或 https:// 开头".to_string());
        }
        if let Some(headers) = config.get("headers") {
            if !headers.is_object()
                || headers
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|v| !v.is_string())
            {
                return Err("headers 必须是字符串键值对象".to_string());
            }
        }
        let endpoints = endpoints_of(config);
        if endpoints.is_empty() {
            return Err("config.endpoints 不能为空（至少登记一个端点）".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for (i, entry) in endpoints.iter().enumerate() {
            let manifest = manifest_for(entry).map_err(|e| format!("endpoints[{i}]: {e}"))?;
            if !seen.insert(manifest.name.clone()) {
                return Err(format!("endpoints[{i}]: tool 名 `{}` 重复", manifest.name));
            }
            let path = entry.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !path.starts_with('/') {
                return Err(format!("endpoints[{i}]: path 必须以 / 开头"));
            }
        }
        Ok(())
    }

    async fn list_tools(&self, instance: &SoftwareInstance) -> Result<Vec<ToolManifest>, String> {
        Ok(endpoints_of(&instance.config)
            .iter()
            .filter_map(|e| manifest_for(e).ok())
            .collect())
    }

    async fn call_tool(
        &self,
        instance: &SoftwareInstance,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let entry = endpoints_of(&instance.config)
            .into_iter()
            .find(|e| e.get("tool").and_then(|v| v.as_str()) == Some(tool))
            .ok_or_else(|| format!("工具不存在: {tool}"))?;

        let base_url = instance
            .config
            .get("base_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string();
        let path = executor::render_template(
            entry.get("path").and_then(|v| v.as_str()).unwrap_or("/"),
            args,
        );
        let method = entry
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        // params 中 in: query 的参数拼到查询串，其余按模板进入 path/body
        let mut query_pairs: Vec<(String, String)> = vec![];
        if let Some(params) = entry.get("params").and_then(|v| v.as_array()) {
            for p in params {
                let is_query = p.get("in").and_then(|v| v.as_str()) == Some("query");
                if !is_query {
                    continue;
                }
                let Some(name) = p.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                if let Some(val) = args.get(name) {
                    let val = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !val.is_empty() {
                        query_pairs.push((name.to_string(), val));
                    }
                }
            }
        }

        let mut url = format!("{base_url}{path}");
        if !query_pairs.is_empty() {
            let qs = query_pairs
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding_encode(k), urlencoding_encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url.push('?');
            url.push_str(&qs);
        }

        let mut headers = headers_of(&instance.config);
        if let Some(extra) = entry.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in extra {
                if let Some(s) = v.as_str() {
                    headers.push((k.clone(), executor::render_template(s, args)));
                }
            }
        }

        let body_template = entry.get("body").and_then(|v| v.as_str());
        let body = body_template.map(|b| executor::render_template(b, args));
        let has_ct = headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
        if body.is_some() && !has_ct {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }

        let timeout = Duration::from_secs(
            entry
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .clamp(1, 300),
        );

        let header_refs: Vec<(String, String)> = headers;
        let outcome = executor::http_request(
            &method,
            &url,
            &header_refs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            body.as_deref(),
            timeout,
        )
        .await;

        let result = serde_json::json!({
            "success": outcome.success,
            "status": outcome.status,
            "url": url,
            "body": outcome.body,
        });
        if outcome.success {
            Ok(result)
        } else {
            Err(serde_json::to_string(&result).unwrap_or_else(|_| outcome.body.clone()))
        }
    }
}

/// 最小 URL 编码（query 值场景），避免引入额外依赖。
fn urlencoding_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(config: serde_json::Value) -> SoftwareInstance {
        SoftwareInstance {
            id: 1,
            connector_id: "generic_http".into(),
            name: "t".into(),
            config,
            mcp_enabled: 1,
            approval_mode: "auto".into(),
            enabled: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn path_placeholder_becomes_required_schema_property() {
        let inst = instance(serde_json::json!({
            "base_url": "http://127.0.0.1:1",
            "endpoints": [
                {
                    "tool": "get_task",
                    "description": "按 ID 查询",
                    "method": "GET",
                    "path": "/tasks/{{taskId}}",
                    "risk": "read_only"
                }
            ]
        }));
        let tools = GenericHttpConnector.list_tools(&inst).await.unwrap();
        assert_eq!(tools.len(), 1);
        let schema = tools[0].input_schema.clone();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("taskId"),
            "占位符应出现在 properties: {schema}"
        );
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "taskId"));
    }

    #[tokio::test]
    async fn camel_case_query_param_kept_in_schema() {
        let inst = instance(serde_json::json!({
            "base_url": "http://127.0.0.1:1",
            "endpoints": [
                {
                    "tool": "list_items",
                    "description": "列表",
                    "method": "GET",
                    "path": "/items",
                    "risk": "read_only",
                    "params": [
                        {"name": "sortBy", "in": "query", "description": "排序字段"}
                    ]
                }
            ]
        }));
        let tools = GenericHttpConnector.list_tools(&inst).await.unwrap();
        let props = tools[0].input_schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("sortBy"),
            "驼峰 query 参数不应被丢弃: {props:?}"
        );
    }
}
