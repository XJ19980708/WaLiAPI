//! 本机软件扫描：枚举已安装应用 + 匹配内置接入预设（用户勾选后一键接入）。

use serde::Serialize;

/// 扫描到的本机软件。
#[derive(Debug, Clone, Serialize)]
pub struct ScannedApp {
    /// 应用名（去扩展名）
    pub name: String,
    /// 可执行路径（macOS: .app 包路径；Windows: 安装目录；Linux: .desktop 路径）
    pub path: String,
    /// 扫描来源（Applications / Program Files / desktop files）
    pub source: String,
    /// 命中内置预设时的建议接入方案（null = 用通用「启动」预设）
    pub suggestion: Option<ConnectorSuggestion>,
}

/// 建议接入方案：连接器 + 预填配置。
#[derive(Debug, Clone, Serialize)]
pub struct ConnectorSuggestion {
    pub connector_id: String,
    pub connector_label: String,
    pub hint: String,
    pub config: serde_json::Value,
}

/// 按应用名匹配内置预设（大小写不敏感的子串匹配）。
fn suggestion_for(app_name: &str) -> Option<ConnectorSuggestion> {
    let lower = app_name.to_lowercase();
    let http = |base_url: &str, tool: &str, desc: &str, path: &str, hint: &str| {
        Some(ConnectorSuggestion {
            connector_id: "generic_http".into(),
            connector_label: "通用本地 HTTP".into(),
            hint: hint.to_string(),
            config: serde_json::json!({
                "base_url": base_url,
                "headers": {},
                "endpoints": [{
                    "tool": tool,
                    "description": desc,
                    "method": "GET",
                    "path": path,
                    "risk": "read_only",
                    "timeout_secs": 20
                }]
            }),
        })
    };

    if lower.contains("ollama") {
        return http(
            "http://127.0.0.1:11434",
            "list_models",
            "列出本地已安装模型",
            "/v1/models",
            "检测到 Ollama，预填本地 API（11434 端口）",
        );
    }
    if lower.contains("qbittorrent") {
        return http(
            "http://127.0.0.1:8080",
            "list_torrents",
            "查询下载任务列表",
            "/api/v2/torrents/info",
            "检测到 qBittorrent，预填 WebUI API（8080 端口，需在 qBittorrent 开启 WebUI）",
        );
    }
    if lower.contains("jupyter") {
        return http(
            "http://127.0.0.1:8888",
            "list_kernels",
            "列出内核规格",
            "/api/kernelspecs",
            "检测到 Jupyter，预填本地 API（8888 端口）",
        );
    }
    if lower.contains("wechat") || lower.contains("微信") {
        return Some(ConnectorSuggestion {
            connector_id: "wechat".into(),
            connector_label: "微信".into(),
            hint: "检测到微信，预填实验性连接器（发送走桌面弹窗审批，需辅助功能权限）".into(),
            config: serde_json::json!({}),
        });
    }
    None
}

/// 未命中预设时的默认方案：通用命令行 + 跨平台「启动该软件」工具。
pub fn launch_preset(name: &str, path: &str) -> ConnectorSuggestion {
    let config = if cfg!(target_os = "macos") {
        serde_json::json!({
            "commands": [{
                "tool": "launch",
                "description": format!("启动 {name}"),
                "program": "open",
                "args": ["-a", name],
                "risk": "read_only",
                "timeout_secs": 15
            }]
        })
    } else if cfg!(target_os = "windows") {
        serde_json::json!({
            "commands": [{
                "tool": "launch",
                "description": format!("启动 {name}"),
                "program": "cmd",
                "args": ["/c", "start", "", path],
                "risk": "read_only",
                "timeout_secs": 15
            }]
        })
    } else {
        serde_json::json!({
            "commands": [{
                "tool": "launch",
                "description": format!("启动 {name}"),
                "program": "xdg-open",
                "args": [path],
                "risk": "read_only",
                "timeout_secs": 15
            }]
        })
    };
    ConnectorSuggestion {
        connector_id: "generic_cli".into(),
        connector_label: "通用命令行".into(),
        hint: "预填「启动软件」命令，可在编辑中追加更多命令/脚本工具".into(),
        config,
    }
}

fn push_app(
    apps: &mut Vec<ScannedApp>,
    seen: &mut std::collections::HashSet<String>,
    name: String,
    path: String,
    source: &str,
) {
    let key = name.to_lowercase();
    if name.trim().is_empty() || !seen.insert(key) {
        return;
    }
    let suggestion = suggestion_for(&name);
    apps.push(ScannedApp {
        name,
        path,
        source: source.to_string(),
        suggestion,
    });
}

/// 扫描本机已安装软件（跨平台 best-effort，失败返回已扫到的部分）。
pub async fn scan_installed_apps() -> Vec<ScannedApp> {
    // 纯文件系统枚举放 blocking 池，避免阻塞 tokio worker
    match tokio::task::spawn_blocking(scan_sync).await {
        Ok(apps) => apps,
        Err(e) => {
            tracing::warn!("[软件] 本机软件扫描失败: {e}");
            vec![]
        }
    }
}

fn scan_sync() -> Vec<ScannedApp> {
    let mut apps: Vec<ScannedApp> = vec![];
    let mut seen = std::collections::HashSet::new();

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let dirs = [
            ("/Applications", "Applications"),
            (&format!("{home}/Applications"), "~/Applications"),
        ];
        for (dir, source) in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let Some(stem) = file_name.strip_suffix(".app") else {
                    continue;
                };
                let path = entry.path().to_string_lossy().to_string();
                push_app(&mut apps, &mut seen, stem.to_string(), path, source);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let candidates = [
            (r"C:\Program Files", "Program Files"),
            (r"C:\Program Files (x86)", "Program Files (x86)"),
            (
                std::env::var("LOCALAPPDATA")
                    .map(|d| format!("{d}\\Programs"))
                    .unwrap_or_default(),
                "User Programs",
            ),
        ];
        for (dir, source) in candidates {
            if dir.is_empty() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path().to_string_lossy().to_string();
                push_app(&mut apps, &mut seen, name, path, source);
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let Ok(entries) = std::fs::read_dir("/usr/share/applications") else {
            return apps;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let Some(stem) = file_name.strip_suffix(".desktop") else {
                continue;
            };
            // 取 Name= 字段（本地化优先级低，够用即可）
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let name = content
                .lines()
                .find_map(|l| l.strip_prefix("Name="))
                .unwrap_or(stem)
                .trim()
                .to_string();
            let path = entry.path().to_string_lossy().to_string();
            push_app(&mut apps, &mut seen, name, path, "Desktop Entries");
        }
    }

    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_known_apps() {
        let ollama = suggestion_for("Ollama").unwrap();
        assert_eq!(ollama.connector_id, "generic_http");
        assert_eq!(ollama.config["base_url"], "http://127.0.0.1:11434");

        let wechat = suggestion_for("WeChat").unwrap();
        assert_eq!(wechat.connector_id, "wechat");
        assert!(suggestion_for("微信").is_some());

        assert!(suggestion_for("Safari").is_none());
    }

    #[test]
    fn launch_preset_has_command() {
        let preset = launch_preset("SomeApp", "/Applications/SomeApp.app");
        assert_eq!(preset.connector_id, "generic_cli");
        assert_eq!(preset.config["commands"][0]["tool"], "launch");
    }

    #[tokio::test]
    async fn scan_returns_results_on_macos() {
        let apps = scan_installed_apps().await;
        // macOS CI/本机必有 /Applications；其他平台允许为空
        if cfg!(target_os = "macos") {
            assert!(!apps.is_empty(), "macOS /Applications 扫描不应为空");
        }
        for app in &apps {
            assert!(!app.name.is_empty());
        }
    }
}
