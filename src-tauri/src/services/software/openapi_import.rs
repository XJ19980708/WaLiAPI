//! OpenAPI / Swagger 规范导入器。
//!
//! 把 OpenAPI 3.x / Swagger 2.0（JSON 或 YAML）文档转换为 [`generic_http`]
//! 连接器配置：每个 operation 生成一个 endpoint（即一个 MCP 工具），让
//! 「接入自带 API 的软件」从手写 JSON 变成粘贴一份规范文档。
//!
//! 转换规则：
//! - 工具名：优先 `operationId`（清洗为 `[a-z0-9_]`），缺失时用 `{method}_{path}` 推导，重名自动加序号
//! - 路径：OpenAPI 的 `{param}` 转为连接器模板 `{{param}}`
//! - 查询参数：`in: query` → 连接器 `{"in": "query"}` 参数
//! - 请求体：存在 body 时登记 `body` 字符串参数，body 模板为 `{{body}}`（调用方传 JSON 字符串）
//! - 风险级别：GET/HEAD → read_only；DELETE → high_write；其余 → low_write
//!
//! 导入结果经 [`generic_http::GenericHttpConnector::validate_config`] 自检后才返回。

use super::connector::SoftwareConnector;
use super::connectors::generic_http::GenericHttpConnector;
use serde::Serialize;
use serde_json::json;

/// 单次导入上限：避免超大规范（网关级 API 有上千 path）一次灌入过多工具。
const MAX_ENDPOINTS: usize = 100;

#[derive(Debug, Serialize)]
pub struct ImportOutcome {
    /// 可直接写入 generic_http 实例 config 的配置（含 base_url + endpoints）
    pub config: serde_json::Value,
    pub tool_count: usize,
    /// 被跳过的 operation 及原因（如非 JSON 请求体、超过上限）
    pub skipped: Vec<String>,
}

/// 解析并转换 OpenAPI/Swagger 文档。`base_url_override` 优先于文档内 servers/host。
pub fn import(spec_text: &str, base_url_override: Option<&str>) -> Result<ImportOutcome, String> {
    let doc: serde_json::Value = parse_spec(spec_text)?;
    let (base_url, version) = resolve_base_url(&doc, base_url_override)?;
    let paths = doc
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .ok_or("文档缺少 paths 字段，不是有效的 OpenAPI/Swagger 规范")?;

    let mut endpoints: Vec<serde_json::Value> = vec![];
    let mut skipped: Vec<String> = vec![];
    let mut used_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut truncated = false;

    'outer: for (path, path_item) in paths {
        let path_item = match path_item.as_object() {
            Some(o) => o,
            None => continue,
        };
        // Path-item 级共享参数（OpenAPI 3.x / Swagger 2.0 均支持），operation 级覆盖同名
        let shared_params = path_item.get("parameters").cloned().unwrap_or(json!([]));
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let Some(op) = path_item.get(method) else {
                continue;
            };
            let op = match op.as_object() {
                Some(o) => o,
                None => continue,
            };
            if endpoints.len() >= MAX_ENDPOINTS {
                truncated = true;
                break 'outer;
            }

            let tool = tool_name(op, method, path, &mut used_names);
            let description = op
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .or_else(|| op.get("description").and_then(serde_json::Value::as_str))
                .unwrap_or(method)
                .chars()
                .take(200)
                .collect::<String>();

            // 合并参数：path-item 级 + operation 级（同名后者优先）
            let mut params: Vec<serde_json::Value> =
                shared_params.as_array().cloned().unwrap_or_default();
            if let Some(op_params) = op.get("parameters").and_then(serde_json::Value::as_array) {
                for p in op_params {
                    let name = p
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    params.retain(|q| {
                        q.get("name").and_then(serde_json::Value::as_str) != Some(name)
                    });
                    params.push(p.clone());
                }
            }

            let mut conn_params: Vec<serde_json::Value> = vec![];
            let mut has_body = false;
            for p in &params {
                let name = p
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let location = p
                    .get("in")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                match location {
                    "query" => conn_params.push(json!({
                        "name": name,
                        "description": p.get("description").and_then(serde_json::Value::as_str).unwrap_or(""),
                        "in": "query",
                    })),
                    "path" => { /* 已通过路径模板 {{name}} 引用 */ }
                    // header/cookie 参数连接器暂不支持模板化，跳过（不阻断导入）
                    _ => {}
                }
            }
            // 请求体：OpenAPI 3.x requestBody / Swagger 2.0 in:body 参数
            if has_request_body(version, op, &params) {
                has_body = true;
                conn_params.push(json!({
                    "name": "body",
                    "description": "JSON 请求体（字符串形式传入）",
                    "required": true,
                }));
            }

            let mut endpoint = json!({
                "tool": tool,
                "description": description,
                "method": method.to_uppercase(),
                "path": path.replace('{', "{{").replace('}', "}}"),
                "risk": risk_of(method),
                "timeout_secs": 30,
            });
            if !conn_params.is_empty() {
                endpoint["params"] = json!(conn_params);
            }
            if has_body {
                endpoint["body"] = json!("{{body}}");
            }
            endpoints.push(endpoint);
        }
    }

    if truncated {
        skipped.push(format!(
            "operation 数量超过 {MAX_ENDPOINTS} 上限，多余部分未导入"
        ));
    }
    if endpoints.is_empty() {
        return Err(
            "规范中未发现可导入的 operation（需要至少一个 get/post/put/patch/delete）".into(),
        );
    }

    let config = json!({ "base_url": base_url, "endpoints": endpoints });
    // 自检：保证导入结果一定能通过连接器校验（坏配置不流出）
    GenericHttpConnector
        .validate_config(&config)
        .map_err(|e| format!("导入结果未通过连接器校验（可能是规范不完整）: {e}"))?;

    let tool_count = endpoints.len();
    Ok(ImportOutcome {
        config,
        tool_count,
        skipped,
    })
}

/// JSON 优先，失败回退 YAML（Swagger 文档常用 YAML）。
fn parse_spec(text: &str) -> Result<serde_json::Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("规范内容为空".into());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        return Ok(v);
    }
    serde_yaml::from_str::<serde_json::Value>(text)
        .map_err(|e| format!("无法按 JSON 或 YAML 解析规范: {e}"))
}

/// (base_url, 规范版本) — version: 2 = Swagger 2.0，3 = OpenAPI 3.x
fn resolve_base_url(doc: &serde_json::Value, over: Option<&str>) -> Result<(String, u8), String> {
    if let Some(o) = over {
        let o = o.trim_end_matches('/');
        if o.starts_with("http://") || o.starts_with("https://") {
            return Ok((
                o.to_string(),
                doc.get("openapi").is_some() as u8 * 3 + doc.get("swagger").is_some() as u8 * 2,
            ));
        }
        return Err("base_url 必须以 http:// 或 https:// 开头".into());
    }
    if let Some(host) = doc.get("host").and_then(serde_json::Value::as_str) {
        // Swagger 2.0
        let scheme = doc
            .get("schemes")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.first())
            .and_then(serde_json::Value::as_str)
            .unwrap_or("https");
        let base_path = doc
            .get("basePath")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        return Ok((
            format!(
                "{}://{}{}",
                scheme,
                host.trim_end_matches('/'),
                base_path.trim_end_matches('/')
            ),
            2,
        ));
    }
    if let Some(server) = doc
        .get("servers")
        .and_then(serde_json::Value::as_array)
        .and_then(|a| a.first())
        .and_then(|s| s.get("url"))
        .and_then(serde_json::Value::as_str)
    {
        // OpenAPI 3.x：servers[0].url 允许相对路径（依赖部署地址），相对时必须手动指定
        if server.starts_with("http://") || server.starts_with("https://") {
            return Ok((server.trim_end_matches('/').to_string(), 3));
        }
        return Err(format!(
            "servers[0].url 是相对路径（`{server}`），请在导入时显式提供 base_url"
        ));
    }
    Err("规范中未找到服务地址（servers[0].url / host），请在导入时提供 base_url".into())
}

/// 工具名：operationId 优先，缺失/非法时由 method+path 推导；重名追加序号。
fn tool_name(
    op: &serde_json::Map<String, serde_json::Value>,
    method: &str,
    path: &str,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let raw = op
        .get("operationId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let mut base = sanitize_tool_name(raw);
    if base.is_empty() {
        base = sanitize_tool_name(&format!("{method}_{}", path.trim_start_matches('/')));
    }
    if base.is_empty() {
        base = format!("api_{method}");
    }
    let mut name = base.clone();
    let mut i = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}

fn sanitize_tool_name(input: &str) -> String {
    let mut out = String::new();
    for (i, c) in input.chars().enumerate() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if c.is_ascii_uppercase() {
            // 驼峰转下划线：大写字母前补 _（首字母除外）
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    // 连接器要求小写字母开头
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("api_{out}")
    } else {
        out
    }
}

fn risk_of(method: &str) -> &'static str {
    match method {
        "get" | "head" | "options" => "read_only",
        "delete" => "high_write",
        _ => "low_write",
    }
}

/// 判断 operation 是否带请求体（OpenAPI 3.x requestBody / Swagger 2.0 in:body）。
/// 有 body 即导入（用 `body` 字符串参数透传），不限制内容类型。
fn has_request_body(
    version: u8,
    op: &serde_json::Map<String, serde_json::Value>,
    params: &[serde_json::Value],
) -> bool {
    if version == 2 {
        return params
            .iter()
            .any(|p| p.get("in").and_then(serde_json::Value::as_str) == Some("body"));
    }
    op.get("requestBody").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENAPI3_JSON: &str = r#"{
      "openapi": "3.0.0",
      "info": {"title": "Pets", "version": "1.0"},
      "servers": [{"url": "http://127.0.0.1:9000/api"}],
      "paths": {
        "/pets/{petId}": {
          "parameters": [{"name": "petId", "in": "path", "required": true, "schema": {"type": "string"}}],
          "get": {
            "operationId": "getPetById",
            "summary": "按 ID 查宠物",
            "parameters": [{"name": "verbose", "in": "query", "schema": {"type": "boolean"}}],
            "responses": {"200": {"description": "ok"}}
          },
          "delete": {
            "operationId": "deletePet",
            "responses": {"204": {"description": "deleted"}}
          }
        },
        "/pets": {
          "post": {
            "operationId": "createPet",
            "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object"}}}},
            "responses": {"201": {"description": "created"}}
          }
        }
      }
    }"#;

    #[test]
    fn imports_openapi3_json() {
        let out = import(OPENAPI3_JSON, None).unwrap();
        assert_eq!(out.tool_count, 3);
        let eps = out.config["endpoints"].as_array().unwrap();
        let get = eps.iter().find(|e| e["tool"] == "get_pet_by_id").unwrap();
        assert_eq!(get["method"], "GET");
        assert_eq!(get["path"], "/pets/{{petId}}");
        assert_eq!(get["risk"], "read_only");
        let params = get["params"].as_array().unwrap();
        assert!(params
            .iter()
            .any(|p| p["name"] == "verbose" && p["in"] == "query"));
        let del = eps.iter().find(|e| e["tool"] == "delete_pet").unwrap();
        assert_eq!(del["risk"], "high_write");
        let post = eps.iter().find(|e| e["tool"] == "create_pet").unwrap();
        assert_eq!(post["body"], "{{body}}");
        assert_eq!(out.config["base_url"], "http://127.0.0.1:9000/api");
    }

    #[test]
    fn imports_swagger2_yaml() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Demo
  version: "1.0"
host: 127.0.0.1:9000
basePath: /v2
schemes:
  - http
paths:
  /status:
    get:
      operationId: getStatus
      responses:
        200:
          description: ok
"#;
        let out = import(yaml, None).unwrap();
        assert_eq!(out.tool_count, 1);
        assert_eq!(out.config["base_url"], "http://127.0.0.1:9000/v2");
        assert_eq!(out.config["endpoints"][0]["tool"], "get_status");
    }

    #[test]
    fn base_url_override_wins() {
        let out = import(OPENAPI3_JSON, Some("http://10.0.0.5:1234/")).unwrap();
        assert_eq!(out.config["base_url"], "http://10.0.0.5:1234");
    }

    #[test]
    fn dedupes_operation_ids() {
        let doc = r#"{
          "openapi": "3.0.0",
          "servers": [{"url": "http://x.example"}],
          "paths": {
            "/a": {"get": {"operationId": "same", "responses": {"200": {"description": "o"}}}},
            "/b": {"get": {"operationId": "same", "responses": {"200": {"description": "o"}}}}
          }
        }"#;
        let out = import(doc, None).unwrap();
        let names: Vec<_> = out.config["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["tool"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["same", "same_2"]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(import("not a spec {{", None).is_err());
        assert!(import("{}", None).is_err());
        // OpenAPI 3 相对 server 地址且未提供 base_url → 报错提示
        let doc = r#"{"openapi":"3.0.0","servers":[{"url":"/api"}],"paths":{"/a":{"get":{"operationId":"a","responses":{}}}}}"#;
        assert!(import(doc, None).is_err());
    }
}
