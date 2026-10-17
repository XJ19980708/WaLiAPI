//! MCP 桥接层：把启用中的软件实例工具动态暴露进 /mcp，并承接 tools/call。
//!
//! 工具命名空间：`software__{slug}_{实例id}__{tool}`，如 `software__wechat_1__list_chats`。
//! slug 由实例名清洗而来（非 ASCII 归一为下划线），实例 id 保证全局唯一不冲突。

use super::approval;
use super::connector::ToolManifest;
use super::connectors;
use super::model::needs_confirmation;
use super::repository::{SoftwareRepository, ToolLogEntry};
use crate::AppState;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NS_PREFIX: &str = "software__";

/// 工具清单缓存 TTL：MCP 聚合 / 微信等连接器的工具发现有进程冷启动开销，
/// 不允许每次 tools/list 都实时拉取；配置变更（hash 不同）立即失效。
const TOOLS_CACHE_TTL: Duration = Duration::from_secs(60);

struct CachedManifests {
    config_hash: u64,
    fetched_at: Instant,
    manifests: Vec<ToolManifest>,
}

fn tools_cache() -> &'static Mutex<HashMap<i64, CachedManifests>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<i64, CachedManifests>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn config_hash(instance: &super::model::SoftwareInstance) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    instance.connector_id.hash(&mut hasher);
    serde_json::to_string(&instance.config)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// 实例工具清单（带 TTL 缓存；拉取失败时降级用旧缓存）。
async fn instance_manifests(
    connector: &'static dyn super::connector::SoftwareConnector,
    instance: &super::model::SoftwareInstance,
) -> Result<Vec<ToolManifest>, String> {
    let hash = config_hash(instance);

    if let Ok(cache) = tools_cache().lock() {
        if let Some(entry) = cache.get(&instance.id) {
            if entry.config_hash == hash && entry.fetched_at.elapsed() < TOOLS_CACHE_TTL {
                return Ok(entry.manifests.clone());
            }
        }
    }

    match connector.list_tools(instance).await {
        Ok(manifests) => {
            if let Ok(mut cache) = tools_cache().lock() {
                cache.insert(
                    instance.id,
                    CachedManifests {
                        config_hash: hash,
                        fetched_at: Instant::now(),
                        manifests: manifests.clone(),
                    },
                );
            }
            Ok(manifests)
        }
        Err(e) => {
            // 拉取失败：降级使用过期缓存（如有），让调用不至于因瞬时故障全部中断
            if let Ok(cache) = tools_cache().lock() {
                if let Some(entry) = cache.get(&instance.id) {
                    if entry.config_hash == hash {
                        tracing::warn!(
                            "[软件] 实例 {} 工具清单刷新失败，使用缓存: {e}",
                            instance.name
                        );
                        return Ok(entry.manifests.clone());
                    }
                }
            }
            Err(e)
        }
    }
}

/// 实例 slug：实例名清洗为 `[a-z0-9_]`，非 ASCII 字符归一为 `_`，末尾拼 `_id` 保证唯一。
pub fn instance_slug(name: &str, id: i64) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if c.is_ascii_uppercase() {
            slug.push(c.to_ascii_lowercase());
        } else {
            slug.push('_');
        }
    }
    let slug = slug.trim_matches('_');
    let base = if slug.is_empty() { "app" } else { slug };
    format!("{base}_{id}")
}

pub fn mcp_tool_name(instance: &super::model::SoftwareInstance, tool: &str) -> String {
    format!(
        "{NS_PREFIX}{}__{}",
        instance_slug(&instance.name, instance.id),
        tool
    )
}

/// 解析 MCP 工具全名 → (instance_id, tool 短名)。
/// tool 短名内部可再含 `__`（如 MCP 聚合的 `fs__read_file`），split_once 取首个分隔。
pub fn parse_mcp_name(mcp_name: &str) -> Option<(i64, String)> {
    let rest = mcp_name.strip_prefix(NS_PREFIX)?;
    let (mid, tool) = rest.split_once("__")?;
    if tool.is_empty() {
        return None;
    }
    let (slug, id) = mid.rsplit_once('_')?;
    let id = id.parse::<i64>().ok()?;
    if slug.is_empty() {
        return None;
    }
    Some((id, tool.to_string()))
}

/// 组装单个 MCP 工具定义。
fn tool_def(
    instance: &super::model::SoftwareInstance,
    manifest: &super::connector::ToolManifest,
) -> serde_json::Value {
    let name = mcp_tool_name(instance, &manifest.name);
    serde_json::json!({
        "name": name,
        "description": format!(
            "[软件·{}] {}（风险级别：{}）",
            instance.name, manifest.description, manifest.risk.label()
        ),
        "inputSchema": manifest.input_schema,
    })
}

/// 当前暴露给 /mcp 的全部软件工具（tools/list 动态组装调用）。
pub async fn list_enabled_tools(pool: &SqlitePool) -> Vec<serde_json::Value> {
    let repo = SoftwareRepository::new(pool.clone());
    let Ok(instances) = repo.list_exposed().await else {
        return vec![];
    };
    let mut out = vec![];
    for instance in instances {
        let Some(connector) = connectors::get(&instance.connector_id) else {
            tracing::warn!(
                "[软件] 实例 {} 引用未知连接器 {}",
                instance.id,
                instance.connector_id
            );
            continue;
        };
        match instance_manifests(connector, &instance).await {
            Ok(manifests) => {
                for m in &manifests {
                    out.push(tool_def(&instance, m));
                }
            }
            Err(e) => tracing::warn!("[软件] 实例 {} 工具清单获取失败: {e}", instance.name),
        }
    }
    out
}

fn summarize_args(args: &serde_json::Value) -> String {
    let s = serde_json::to_string(args).unwrap_or_default();
    if s.len() > 512 {
        format!("{}…", &s[..512])
    } else {
        s
    }
}

fn mcp_text(result: &serde_json::Value) -> serde_json::Value {
    let text = serde_json::to_string_pretty(result).unwrap_or_default();
    serde_json::json!({"content":[{"type":"text","text":text}],"isError":false})
}

/// tools/call 分发入口（handle_tool_call 检测到 `software__` 前缀后转入）。
pub async fn call_tool(
    state: &AppState,
    mcp_name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let started = std::time::Instant::now();
    let (instance_id, tool) =
        parse_mcp_name(mcp_name).ok_or_else(|| format!("非法软件工具名: {mcp_name}"))?;

    let repo = SoftwareRepository::new(state.db.pool.clone());
    let instance = repo.get(instance_id).await?;
    if instance.enabled != 1 || instance.mcp_enabled != 1 {
        return Err(format!("软件实例 `{}` 未启用或未开启 MCP", instance.name));
    }
    let connector = connectors::get(&instance.connector_id)
        .ok_or_else(|| format!("未知连接器: {}", instance.connector_id))?;

    // 风险级别以工具清单为准（带 TTL 缓存），清单中找不到的工具一律拒绝
    let manifests = instance_manifests(connector, &instance).await?;
    let manifest = manifests
        .iter()
        .find(|m| m.name == tool)
        .ok_or_else(|| format!("工具不存在或未登记: {tool}"))?;

    let risk_str = manifest.risk.as_str();
    let duration_ms = started.elapsed().as_millis() as i64;
    let args_summary = summarize_args(args);
    let entry = |status: &str, error: &str| ToolLogEntry {
        tool: tool.to_string(),
        risk: risk_str.to_string(),
        args_summary: args_summary.clone(),
        status: status.to_string(),
        error: error.to_string(),
        duration_ms,
    };

    // 审批闸门
    if needs_confirmation(&instance.approval_mode, manifest.risk) {
        let confirmed = approval::request_confirmation(
            &state.app_handle,
            "WaLiAPI · 软件操作确认",
            &format!(
                "AI 请求执行高风险软件操作：\n\n软件：{}\n工具：{}（{}）\n\n是否允许？",
                instance.name,
                manifest.name,
                manifest.risk.label()
            ),
        )
        .await;
        if !confirmed {
            repo.insert_tool_log(&instance, &entry("denied", "用户拒绝或环境不支持弹窗确认"))
                .await;
            return Err(format!(
                "操作被拒绝：工具 `{}` 属于{}，需要用户确认。桌面端会弹出确认窗；headless 部署请把实例审批模式改为 auto。",
                tool,
                manifest.risk.label()
            ));
        }
    }

    match connector.call_tool(&instance, &tool, args).await {
        Ok(result) => {
            repo.insert_tool_log(&instance, &entry("ok", "")).await;
            Ok(mcp_text(&result))
        }
        Err(e) => {
            repo.insert_tool_log(&instance, &entry("error", &e)).await;
            Err(e)
        }
    }
}

/// 连接器市场数据源（前端管理页）。
pub fn connector_catalog() -> Vec<serde_json::Value> {
    connectors::all()
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id(),
                "label": c.label(),
                "description": c.description(),
                "platforms": c.platforms(),
                "sample_config": c.sample_config(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_parse_roundtrip() {
        let inst = super::super::model::SoftwareInstance {
            id: 7,
            connector_id: "generic_cli".into(),
            name: "微信".into(),
            config: serde_json::json!({}),
            mcp_enabled: 1,
            approval_mode: "auto".into(),
            enabled: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let name = mcp_tool_name(&inst, "list_chats");
        assert_eq!(name, "software__app_7__list_chats");
        let (id, tool) = parse_mcp_name(&name).unwrap();
        assert_eq!(id, 7);
        assert_eq!(tool, "list_chats");
    }

    #[test]
    fn slug_ascii_keeps_name() {
        let inst = super::super::model::SoftwareInstance {
            id: 3,
            connector_id: "generic_cli".into(),
            name: "Ollama Local".into(),
            config: serde_json::json!({}),
            mcp_enabled: 1,
            approval_mode: "auto".into(),
            enabled: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let name = mcp_tool_name(&inst, "list_models");
        assert_eq!(name, "software__ollama_local_3__list_models");
        assert!(parse_mcp_name(&name).is_some());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_mcp_name("search_knowledge_base").is_none());
        assert!(parse_mcp_name("software__abc__x").is_none());
        assert!(parse_mcp_name("software__a_1__").is_none());
    }

    #[test]
    fn aggregate_tool_names_roundtrip() {
        let inst = super::super::model::SoftwareInstance {
            id: 3,
            connector_id: "generic_mcp".into(),
            name: "MCP".into(),
            config: serde_json::json!({}),
            mcp_enabled: 1,
            approval_mode: "auto".into(),
            enabled: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let name = mcp_tool_name(&inst, "fs__read_file");
        assert_eq!(name, "software__mcp_3__fs__read_file");
        let (id, tool) = parse_mcp_name(&name).unwrap();
        assert_eq!(id, 3);
        assert_eq!(tool, "fs__read_file");
    }
}
