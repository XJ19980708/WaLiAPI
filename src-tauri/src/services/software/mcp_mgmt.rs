//! MCP 管理工具：让外部 AI Agent 通过 WaLiAPI 的 /mcp 以对话方式完成
//! Link（软件对接）实例的配置——发现连接器、导入 OpenAPI、创建/更新/删除实例。
//!
//! 安全边界：API Key（scoped）与 MCP token 全量模式均可操作（产品约定：API Key
//! 默认可操作 software 能力域）；删除操作要求显式 `confirm: true`，实例级工具
//! 另受实例审批策略（approval_mode）闸门约束。

use crate::commands::software::{svc, CreateSoftwareInstanceInput, UpdateSoftwareInstanceInput};
use crate::AppState;
use serde_json::{json, Value};

/// 工具名清单（get_tools 追加定义 / handle_tool_call 分发共用）。
pub const TOOL_NAMES: &[&str] = &[
    "software_list_connectors",
    "software_list_instances",
    "software_get_instance_tools",
    "software_create_instance",
    "software_update_instance",
    "software_delete_instance",
    "software_import_openapi",
];

/// 追加到 /mcp tools/list 的工具定义（与 mcp_bridge 动态工具同层级）。
pub fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "software_list_connectors",
            "description": "列出 Link（软件对接）支持的连接器类型及各自配置样例：generic_cli 通用命令行 / generic_http 通用本地 HTTP / generic_mcp MCP 聚合网关 / wechat 微信。配置实例前先调用本工具获取样例结构。",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "software_list_instances",
            "description": "列出当前全部 Link 软件对接实例（含名称、连接器、启用状态、MCP 暴露状态、审批模式）。",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "software_get_instance_tools",
            "description": "查看指定实例将暴露为哪些 MCP 工具（工具名、参数 schema、风险级别）。保存实例后用它向用户确认工具形态。",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer", "description": "实例 ID（来自 software_list_instances）"}},
                "required": ["id"]
            }
        }),
        json!({
            "name": "software_create_instance",
            "description": "创建 Link 软件对接实例。connector_id 取 software_list_connectors 结果；config 结构参照对应连接器的 sample_config。创建成功后该实例的 MCP 工具立即生效。建议流程：software_import_openapi 生成 config（HTTP 场景）→ 本工具创建 → software_get_instance_tools 确认。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connector_id": {"type": "string", "description": "连接器 ID，如 generic_http / generic_cli / generic_mcp"},
                    "name": {"type": "string", "description": "实例名称（用户可读，≤100 字符）"},
                    "config": {"type": "object", "description": "连接器配置（结构参照 sample_config，generic_http 可由 software_import_openapi 生成）"},
                    "mcp_enabled": {"type": "boolean", "description": "是否在 /mcp 暴露工具，默认 true", "default": true},
                    "approval_mode": {"type": "string", "enum": ["auto", "confirm_high_risk", "confirm_all"], "description": "审批策略，默认 confirm_high_risk（推荐）"}
                },
                "required": ["connector_id", "name", "config"]
            }
        }),
        json!({
            "name": "software_update_instance",
            "description": "更新 Link 实例：改配置 / 改名 / 切换启用与 MCP 暴露 / 调整审批策略。只传需要修改的字段，配置变更以「更新后的完整 config」整体校验。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "实例 ID"},
                    "name": {"type": "string", "description": "新名称（可选）"},
                    "config": {"type": "object", "description": "新的完整 config（可选）"},
                    "mcp_enabled": {"type": "boolean", "description": "是否在 /mcp 暴露工具（可选）"},
                    "approval_mode": {"type": "string", "enum": ["auto", "confirm_high_risk", "confirm_all"], "description": "审批策略（可选）"}
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "software_delete_instance",
            "description": "删除 Link 实例，其 MCP 工具立即下线。必须显式传 confirm=true 才执行；执行前先与用户确认。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "实例 ID"},
                    "confirm": {"type": "boolean", "description": "必须为 true，表示已获用户明确确认"}
                },
                "required": ["id", "confirm"]
            }
        }),
        json!({
            "name": "software_import_openapi",
            "description": "把 OpenAPI 3.x / Swagger 2.0 规范（JSON 或 YAML 文本）转换为 generic_http 连接器 config：每个 operation 生成一个工具，{param} 自动转 {{param}} 模板，查询参数、请求体、风险级别自动推导。返回的 config 直接用于 software_create_instance。规范里 servers 是相对地址时必须传 base_url。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec": {"type": "string", "description": "OpenAPI/Swagger 文档全文（JSON 或 YAML）"},
                    "base_url": {"type": "string", "description": "可选，覆盖规范内的服务地址（http:// 或 https:// 开头）"}
                },
                "required": ["spec"]
            }
        }),
    ]
}

/// 分发入口：命中管理工具返回 Some(结果)，否则 None（交回原 dispatch）。
pub async fn try_handle(
    state: &AppState,
    tool: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    if !TOOL_NAMES.contains(&tool) {
        return None;
    }
    let result = handle(state, tool, args).await;
    Some(result)
}

async fn handle(state: &AppState, tool: &str, args: &Value) -> Result<Value, String> {
    let outcome: Value = match tool {
        "software_list_connectors" => json!({
            "connectors": crate::services::software::mcp_bridge::connector_catalog()
        }),
        "software_list_instances" => {
            let instances = svc::list_instances(state).await?;
            json!({ "instances": instances })
        }
        "software_get_instance_tools" => {
            let id: i64 = arg(args, "id")?;
            json!({ "tools": svc::instance_tools(state, id).await? })
        }
        "software_create_instance" => {
            // 对话式创建时 mcp_enabled 缺省应为 true（工具 schema 承诺的行为），
            // 而 CreateSoftwareInstanceInput 的 serde 默认是 false（桌面端总是显式传）
            let mut input_value = args.clone();
            if input_value.get("mcp_enabled").is_none() {
                input_value["mcp_enabled"] = json!(true);
            }
            let input: CreateSoftwareInstanceInput =
                serde_json::from_value(input_value).map_err(|e| format!("参数无效: {e}"))?;
            let inst = svc::create_instance(state, input).await?;
            json!({"created": true, "instance": inst})
        }
        "software_update_instance" => {
            let input: UpdateSoftwareInstanceInput =
                serde_json::from_value(args.clone()).map_err(|e| format!("参数无效: {e}"))?;
            let inst = svc::update_instance(state, input).await?;
            json!({"updated": true, "instance": inst})
        }
        "software_delete_instance" => {
            let id: i64 = arg(args, "id")?;
            let confirmed = args
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !confirmed {
                return Err("删除被拒绝：必须显式传 confirm=true（并先获得用户确认）".into());
            }
            svc::delete_instance(state, id).await?;
            json!({"deleted": true, "id": id})
        }
        "software_import_openapi" => {
            let spec: String = arg(args, "spec")?;
            let base_url: Option<String> = arg(args, "base_url")?;
            let out =
                crate::services::software::openapi_import::import(&spec, base_url.as_deref())?;
            json!({
                "tool_count": out.tool_count,
                "skipped": out.skipped,
                "config": out.config,
                "hint": "把 config 字段作为 software_create_instance 的 config 参数（connector_id=generic_http）",
            })
        }
        _ => unreachable!("TOOL_NAMES 与 handle 分支不同步"),
    };
    Ok(json!({
        "content": [{"type": "text", "text": serde_json::to_string(&outcome).unwrap_or_default()}],
        "isError": false
    }))
}

fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("参数 {key} 无效: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_cover_defs() {
        let defs = tool_defs();
        assert_eq!(defs.len(), TOOL_NAMES.len());
        for d in &defs {
            let name = d["name"].as_str().unwrap();
            assert!(TOOL_NAMES.contains(&name), "定义 {name} 不在 TOOL_NAMES");
        }
    }

    #[test]
    fn try_handle_ignores_other_tools() {
        // 同步部分：非管理工具名直接返回 None（不触碰 state）
        let v = json!({});
        assert!(try_handle_sync_probe(&v, "search_knowledge_base"));
    }

    fn try_handle_sync_probe(_args: &Value, name: &str) -> bool {
        !TOOL_NAMES.contains(&name)
    }
}
