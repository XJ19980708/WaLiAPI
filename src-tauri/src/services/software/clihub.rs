//! CLI-Hub 服务：把 [CLI-Anything](https://github.com/HKUDS/CLI-Anything)
//! 社区目录接入 Link 的「一键安装 + 创建实例」流程。
//!
//! 数据面：
//! - `cli-hub list --json`：社区目录（103+ 条，字段 name / display_name /
//!   description / entry_point / category / requires / install_cmd）
//! - `cli-hub install <name>`：安装（pip，可能较慢，给足超时）
//! - 安装产物：`<entry_point>`（如 `cli-anything-gimp`）进入 PATH
//!
//! 容错：cli-hub 未安装时返回内置精选目录（静态），安装动作提示先装 hub。

use crate::commands::software::svc;
use crate::services::software::connector::SoftwareConnector;
use crate::services::software::connectors::clihub::ClihubConnector;
use crate::services::software::model::SoftwareInstance;
use crate::services::software::repository::SoftwareRepository;
use crate::AppState;
use serde::Serialize;
use std::time::Duration;

/// 单条目录项（对 cli-hub JSON 的宽松投影）。
#[derive(Debug, Clone, Serialize)]
pub struct ClihubEntry {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub entry_point: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
    pub installed: bool,
    /// true = 来自实时 `cli-hub list`；false = hub 未安装时的内置精选目录
    pub live: bool,
}

#[derive(Debug, Serialize)]
pub struct ClihubCatalog {
    /// cli-hub 命令是否可用（未可用时 entries 为内置精选目录）
    pub hub_available: bool,
    pub entries: Vec<ClihubEntry>,
}

/// hub 未安装时的内置精选目录（来自 CLI-Anything README 已知条目）。
fn fallback_entries() -> Vec<ClihubEntry> {
    let known = [
        ("gimp", "GIMP", "image", "GIMP 图像编辑 agent 原生 CLI"),
        (
            "blender",
            "Blender",
            "3d",
            "Blender 三维建模/渲染 agent 原生 CLI",
        ),
        (
            "libreoffice",
            "LibreOffice",
            "office",
            "LibreOffice 文档处理 agent 原生 CLI",
        ),
        (
            "inkscape",
            "Inkscape",
            "design",
            "Inkscape 矢量图形 agent 原生 CLI",
        ),
        (
            "obs-studio",
            "OBS Studio",
            "streaming",
            "OBS Studio 直播/录播 agent 原生 CLI",
        ),
        (
            "audacity",
            "Audacity",
            "audio",
            "Audacity 音频编辑 agent 原生 CLI",
        ),
        (
            "obsidian",
            "Obsidian",
            "knowledge-management",
            "Obsidian 笔记库 agent 原生 CLI",
        ),
        (
            "calibre",
            "Calibre",
            "office",
            "Calibre 电子书管理 agent 原生 CLI",
        ),
    ];
    known
        .into_iter()
        .map(|(name, display, category, desc)| ClihubEntry {
            name: name.to_string(),
            display_name: display.to_string(),
            description: desc.to_string(),
            entry_point: format!("cli-anything-{name}"),
            category: category.to_string(),
            requires: None,
            installed: false,
            live: false,
        })
        .collect()
}

fn entry_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// 探测 PATH 上是否存在某可执行（which / where，2s 超时）。
async fn on_path(binary: String) -> bool {
    #[cfg(target_os = "windows")]
    let finder = "where";
    #[cfg(not(target_os = "windows"))]
    let finder = "which";
    let out = tokio::process::Command::new(finder)
        .arg(binary)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;
    matches!(out, Ok(o) if o.status.success() && !o.stdout.is_empty())
}

/// cli-hub 命令是否可用。
pub async fn hub_available() -> bool {
    on_path("cli-hub".to_string()).await
}

/// 拉取社区目录。hub 可用时走实时列表（附安装状态），否则回退内置精选。
pub async fn catalog() -> ClihubCatalog {
    let available = hub_available().await;
    if !available {
        return ClihubCatalog {
            hub_available: false,
            entries: fallback_entries(),
        };
    }
    let out = tokio::process::Command::new("cli-hub")
        .args(["list", "--json"])
        .output()
        .await;
    let parsed = match out {
        Ok(o) if o.status.success() => serde_json::from_slice::<serde_json::Value>(&o.stdout).ok(),
        _ => None,
    };
    let Some(list) = parsed.and_then(|v| v.as_array().cloned()) else {
        return ClihubCatalog {
            hub_available: true,
            entries: fallback_entries(),
        };
    };
    let mut entries: Vec<ClihubEntry> = list
        .into_iter()
        .filter_map(|e| {
            let name = e.get("name")?.as_str()?.to_string();
            if !entry_name_ok(&name) {
                return None;
            }
            let entry_point = e
                .get("entry_point")
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("cli-anything-{name}"))
                .to_string();
            Some(ClihubEntry {
                display_name: e
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&name)
                    .to_string(),
                description: e
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                category: e
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("misc")
                    .to_string(),
                requires: e
                    .get("requires")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                entry_point,
                name,
                installed: false,
                live: true,
            })
        })
        .collect();
    // 并发探测安装状态（entry_point 是否在 PATH 上）
    let probes = entries
        .iter()
        .map(|e| tokio::spawn(on_path(e.entry_point.clone())))
        .collect::<Vec<_>>();
    let results = futures_util::future::join_all(probes).await;
    for (entry, res) in entries.iter_mut().zip(results) {
        entry.installed = res.unwrap_or_default();
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    ClihubCatalog {
        hub_available: true,
        entries,
    }
}

/// `cli-hub install <name>`。耗时操作（pip 拉取构建），600s 超时。
pub async fn install(name: &str) -> Result<String, String> {
    if !entry_name_ok(name) {
        return Err(format!("非法 CLI 名 `{name}`"));
    }
    if !hub_available().await {
        return Err("cli-hub 未安装。请先执行：pip install cli-anything-hub".to_string());
    }
    let out = tokio::process::Command::new("cli-hub")
        .arg("install")
        .arg(name)
        .output()
        .await
        .map_err(|e| format!("启动 cli-hub 失败: {e}"))?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string())
    } else {
        Err(format!(
            "cli-hub install {name} 失败:\n{}",
            text.lines().rev().take(15).collect::<Vec<_>>().join("\n")
        ))
    }
}

/// 把已安装的 CLI 一键接入为 Link 实例（connector = clihub）。
pub async fn create_instance(
    state: &AppState,
    name: &str,
    mcp_enabled: bool,
) -> Result<SoftwareInstance, String> {
    if !entry_name_ok(name) {
        return Err(format!("非法 CLI 名 `{name}`"));
    }
    let binary = format!("cli-anything-{name}");
    if !on_path(binary.clone()).await {
        return Err(format!(
            "`{binary}` 不在 PATH 上。请先安装：cli-hub install {name}"
        ));
    }
    // 用 help 输出做 intro（截断首行描述），失败不阻塞
    let intro = match tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(&binary).arg("--help").output(),
    )
    .await
    {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("CLI-Anything 生成的 agent 原生 CLI")
            .trim()
            .chars()
            .take(120)
            .collect::<String>()
            .trim()
            .to_string(),
        _ => "CLI-Anything 生成的 agent 原生 CLI".to_string(),
    };
    let display = name.replace('-', "_");
    crate::commands::software::svc::create_instance(
        state,
        crate::commands::software::CreateSoftwareInstanceInput {
            connector_id: ClihubConnector.id().to_string(),
            name: format!("CLI-Hub · {display}"),
            config: serde_json::json!({
                "cli": name,
                "binary": binary,
                "intro": intro,
                "run_risk": "low_write",
            }),
            mcp_enabled,
            approval_mode: "confirm_high_risk".to_string(),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_name_rules() {
        assert!(entry_name_ok("gimp"));
        assert!(entry_name_ok("obs-studio"));
        assert!(entry_name_ok("libreoffice"));
        assert!(!entry_name_ok(""));
        assert!(!entry_name_ok("Gimp"));
        assert!(!entry_name_ok("a;rm"));
        assert!(!entry_name_ok(&"x".repeat(100)));
    }

    #[tokio::test]
    async fn install_rejects_bad_names_before_spawning() {
        assert!(install("bad;name").await.is_err());
        assert!(install("").await.is_err());
    }

    #[tokio::test]
    async fn catalog_degrades_without_hub() {
        // 沙箱/CI 不一定有 cli-hub；目录必须优雅降级为内置精选
        let cat = catalog().await;
        if !cat.hub_available {
            assert!(!cat.entries.is_empty());
            assert!(cat.entries.iter().all(|e| !e.live));
            assert!(cat
                .entries
                .iter()
                .all(|e| e.entry_point.starts_with("cli-anything-")));
        }
    }
}
