//! 软件对接管理命令（桌面 Tauri invoke / headless /admin/api/invoke 共用）。

use crate::services::software::connectors;
use crate::services::software::model::{self, SoftwareInstance};
use crate::services::software::repository::SoftwareRepository;
use crate::services::software::scan::{self, ScannedApp};
use crate::AppState;
use serde::Deserialize;
use std::sync::Arc;
use tauri::State;

#[derive(Debug, Deserialize)]
pub struct CreateSoftwareInstanceInput {
    pub connector_id: String,
    pub name: String,
    pub config: serde_json::Value,
    #[serde(default)]
    pub mcp_enabled: bool,
    #[serde(default = "default_approval")]
    pub approval_mode: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSoftwareInstanceInput {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub mcp_enabled: Option<bool>,
    #[serde(default)]
    pub approval_mode: Option<String>,
}

fn default_approval() -> String {
    model::APPROVAL_CONFIRM_HIGH_RISK.to_string()
}

fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("实例名称不能为空".into());
    }
    if trimmed.len() > 100 {
        return Err("实例名称过长（≤100 字符）".into());
    }
    Ok(())
}

fn validate_input(
    connector_id: &str,
    name: &str,
    config: &serde_json::Value,
    approval_mode: &str,
) -> Result<(), String> {
    validate_name(name)?;
    let Some(connector) = connectors::get(connector_id) else {
        return Err(format!("未知连接器: {connector_id}"));
    };
    if !model::valid_approval_mode(approval_mode) {
        return Err(format!(
            "非法审批模式 `{approval_mode}`（auto / confirm_high_risk / confirm_all）"
        ));
    }
    connector.validate_config(config)
}

/// 服务层：以 `&AppState` 为参数的核心实现，供 Tauri command 与 MCP 管理工具
/// （services/software/mcp_mgmt.rs）复用，避免两处分发逻辑漂移。
pub mod svc {
    use super::*;

    pub async fn list_instances(state: &AppState) -> Result<Vec<SoftwareInstance>, String> {
        SoftwareRepository::new(state.db.pool.clone())
            .get_all()
            .await
    }

    pub async fn create_instance(
        state: &AppState,
        input: CreateSoftwareInstanceInput,
    ) -> Result<SoftwareInstance, String> {
        validate_input(
            &input.connector_id,
            &input.name,
            &input.config,
            &input.approval_mode,
        )?;
        SoftwareRepository::new(state.db.pool.clone())
            .create(
                &input.connector_id,
                input.name.trim(),
                &input.config,
                input.mcp_enabled,
                &input.approval_mode,
            )
            .await
    }

    pub async fn update_instance(
        state: &AppState,
        input: UpdateSoftwareInstanceInput,
    ) -> Result<SoftwareInstance, String> {
        let repo = SoftwareRepository::new(state.db.pool.clone());
        let existing = repo.get(input.id).await?;

        // 合并校验：以「更新后的最终状态」校验配置
        let final_name = input.name.clone().unwrap_or_else(|| existing.name.clone());
        let final_config = input
            .config
            .clone()
            .unwrap_or_else(|| existing.config.clone());
        let final_approval = input
            .approval_mode
            .clone()
            .unwrap_or_else(|| existing.approval_mode.clone());
        validate_input(
            &existing.connector_id,
            &final_name,
            &final_config,
            &final_approval,
        )?;

        repo.update(
            input.id,
            input.name.as_deref().map(str::trim),
            input.config.as_ref(),
            input.mcp_enabled,
            input.approval_mode.as_deref(),
        )
        .await
    }

    pub async fn delete_instance(state: &AppState, id: i64) -> Result<(), String> {
        SoftwareRepository::new(state.db.pool.clone())
            .delete(id)
            .await
    }

    pub async fn instance_tools(
        state: &AppState,
        id: i64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let repo = SoftwareRepository::new(state.db.pool.clone());
        let instance = repo.get(id).await?;
        let Some(connector) = connectors::get(&instance.connector_id) else {
            return Err(format!("未知连接器: {}", instance.connector_id));
        };
        let manifests = connector.list_tools(&instance).await?;
        Ok(manifests
            .into_iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "mcp_name": crate::services::software::mcp_bridge::mcp_tool_name(&instance, &m.name),
                    "description": m.description,
                    "input_schema": m.input_schema,
                    "risk": m.risk.as_str(),
                    "risk_label": m.risk.label(),
                })
            })
            .collect())
    }
}

#[tauri::command]
pub async fn get_software_instances(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<SoftwareInstance>, String> {
    svc::list_instances(state.inner()).await
}

/// 连接器市场：内置连接器清单 + 配置示例。
#[tauri::command]
pub async fn get_software_connector_catalog() -> Result<Vec<serde_json::Value>, String> {
    Ok(mcp_bridge_catalog())
}

fn mcp_bridge_catalog() -> Vec<serde_json::Value> {
    crate::services::software::mcp_bridge::connector_catalog()
}

#[tauri::command]
pub async fn create_software_instance(
    state: State<'_, Arc<AppState>>,
    input: CreateSoftwareInstanceInput,
) -> Result<SoftwareInstance, String> {
    svc::create_instance(state.inner(), input).await
}

#[tauri::command]
pub async fn update_software_instance(
    state: State<'_, Arc<AppState>>,
    input: UpdateSoftwareInstanceInput,
) -> Result<SoftwareInstance, String> {
    svc::update_instance(state.inner(), input).await
}

#[tauri::command]
pub async fn toggle_software_instance(
    state: State<'_, Arc<AppState>>,
    id: i64,
    enabled: bool,
) -> Result<SoftwareInstance, String> {
    let repo = SoftwareRepository::new(state.db.pool.clone());
    repo.toggle(id, enabled).await
}

#[tauri::command]
pub async fn delete_software_instance(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<(), String> {
    svc::delete_instance(state.inner(), id).await
}

/// 实例工具预览（保存后即可看到将暴露哪些 MCP 工具）。
#[tauri::command]
pub async fn get_software_instance_tools(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<Vec<serde_json::Value>, String> {
    svc::instance_tools(state.inner(), id).await
}

#[tauri::command]
pub async fn get_software_tool_logs(
    state: State<'_, Arc<AppState>>,
    instance_id: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    let repo = SoftwareRepository::new(state.db.pool.clone());
    repo.list_tool_logs(instance_id, limit.unwrap_or(50)).await
}

/// 扫描本机已安装软件 + 匹配内置接入预设（用户勾选后接入）。
#[tauri::command]
pub async fn scan_installed_software() -> Result<Vec<ScannedApp>, String> {
    Ok(scan::scan_installed_apps().await)
}

/// OpenAPI/Swagger 规范导入：纯转换函数（无状态），把规范文本转换为
/// generic_http 连接器 config。前端编辑弹窗与 MCP 管理工具共用。
#[tauri::command]
pub async fn import_openapi(
    spec: String,
    base_url: Option<String>,
) -> Result<serde_json::Value, String> {
    let out = crate::services::software::openapi_import::import(&spec, base_url.as_deref())?;
    serde_json::to_value(out).map_err(|e| e.to_string())
}
