//! 软件对接能力域 —— 把本地软件的操作能力以 MCP 工具形式暴露。
//!
//! 分层：
//! - [`connector`]：`SoftwareConnector` trait，每个软件/通用方案一个连接器
//! - [`connectors`]：内置连接器（generic_cli / generic_http）
//! - [`executor`]：执行底座（白名单子进程、脚本桥、HTTP）
//! - [`approval`]：高危操作确认（桌面弹窗）
//! - [`repository`]：software_instances / software_tool_logs 仓储
//! - [`mcp_bridge`]：MCP 层桥接（动态 tools/list + tools/call 分发）

pub mod approval;
pub mod clihub;
pub mod connector;
pub mod connectors;
pub mod executor;
pub mod mcp_bridge;
pub mod mcp_client;
pub mod mcp_mgmt;
pub mod model;
pub mod openapi_import;
pub mod repository;
pub mod scan;

use super::{Service, ServiceStatus};
use crate::server::router::SharedState;
use crate::AppState;
use async_trait::async_trait;
use axum::Router;
use std::sync::Arc;

pub struct SoftwareService;

#[async_trait]
impl Service for SoftwareService {
    fn id(&self) -> &'static str {
        "software"
    }
    fn name(&self) -> &'static str {
        "Software"
    }
    fn description(&self) -> &'static str {
        "软件对接：本地软件操作能力，经连接器（CLI/HTTP/脚本桥/UI 自动化）封装为 MCP 工具暴露给 AI Agent"
    }

    async fn status(&self, state: &Arc<AppState>) -> ServiceStatus {
        let repo = repository::SoftwareRepository::new(state.db.pool.clone());
        let instances = repo.get_all().await.unwrap_or_default();
        let enabled_count = instances.iter().filter(|i| i.enabled == 1).count();
        let mcp_count = instances
            .iter()
            .filter(|i| i.enabled == 1 && i.mcp_enabled == 1)
            .count();

        // 汇总当前暴露的工具名（与 /mcp tools/list 一致）
        let tools = mcp_bridge::list_enabled_tools(&state.db.pool)
            .await
            .into_iter()
            .map(|tool| {
                let name = tool
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                let description = tool
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                serde_json::json!({ "name": name, "label": name, "desc": description })
            })
            .collect::<Vec<_>>();

        ServiceStatus {
            id: self.id().to_string(),
            name: self.name().to_string(),
            description: self.description().to_string(),
            enabled: true,
            running: true,
            stats: serde_json::json!({
                "total_instances": instances.len(),
                "enabled_instances": enabled_count,
                "mcp_enabled_instances": mcp_count,
                "tools": tools,
            }),
        }
    }

    /// 管理面走 Tauri commands（桌面）/ /admin/api/invoke（headless），无独立数据面路由。
    fn routes(&self, _state: Arc<AppState>) -> Router<SharedState> {
        Router::new()
    }
}
