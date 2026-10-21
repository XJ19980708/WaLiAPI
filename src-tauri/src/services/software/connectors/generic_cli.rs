//! 通用命令行连接器。
//!
//! 用户把一条条「程序 + 参数模板」登记为 MCP 工具：
//! - `program` 直接 exec（不经 shell），`args` 中可用 `{{param}}` 整值替换
//! - `program` 填 `osascript`（macOS）/ `powershell`（Windows）即脚本桥用法
//! - `risk` 显式声明，高危工具由审批策略把关
//!
//! 配置示例：
//! ```json
//! {
//!   "commands": [
//!     {
//!       "tool": "battery",
//!       "description": "查询笔记本电量",
//!       "program": "pmset",
//!       "args": ["-g", "batt"],
//!       "risk": "read_only",
//!       "timeout_secs": 15
//!     }
//!   ]
//! }
//! ```

use super::super::connector::{RiskLevel, ToolManifest};
use super::super::executor;
use super::super::model::SoftwareInstance;
use super::{build_input_schema, param_names, valid_tool_name};
use crate::services::software::connector::SoftwareConnector;
use async_trait::async_trait;
use std::time::Duration;

pub struct GenericCliConnector;

fn commands_of(config: &serde_json::Value) -> Vec<serde_json::Value> {
    config
        .get("commands")
        .and_then(|v| v.as_array())
        .cloned()
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
    let description = entry
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or(name);
    let risk = RiskLevel::parse(entry.get("risk").and_then(|v| v.as_str()).unwrap_or(""));
    let params = entry
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    Ok(ToolManifest {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: build_input_schema(&params),
        risk,
    })
}

#[async_trait]
impl SoftwareConnector for GenericCliConnector {
    fn id(&self) -> &'static str {
        "generic_cli"
    }
    fn label(&self) -> &'static str {
        "通用命令行"
    }
    fn description(&self) -> &'static str {
        "把本地命令/脚本登记为 MCP 工具（CLI、AppleScript、PowerShell 通用）。程序直接执行不经 shell，支持 {{参数}} 模板"
    }
    fn platforms(&self) -> &'static str {
        "macOS / Windows / Linux"
    }
    fn sample_config(&self) -> serde_json::Value {
        serde_json::json!({
            "commands": [
                {
                    "tool": "battery",
                    "description": "查询笔记本电量",
                    "program": "pmset",
                    "args": ["-g", "batt"],
                    "risk": "read_only",
                    "timeout_secs": 15
                },
                {
                    "tool": "say_hello",
                    "description": "macOS 语音播报（脚本桥示例）",
                    "program": "osascript",
                    "args": ["-e", "say {{text}}"],
                    "params": [{"name": "text", "description": "要播报的内容", "required": true}],
                    "risk": "low_write",
                    "timeout_secs": 30
                },
                {
                    "tool": "notify",
                    "description": "Windows 弹通知（脚本桥示例）",
                    "program": "powershell",
                    "args": ["-NoProfile", "-NonInteractive", "-Command",
                             "[System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms'); [System.Windows.Forms.MessageBox]::Show('{{text}}')"],
                    "params": [{"name": "text", "description": "通知内容", "required": true}],
                    "risk": "low_write",
                    "timeout_secs": 60
                }
            ]
        })
    }

    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String> {
        let commands = commands_of(config);
        if commands.is_empty() {
            return Err("config.commands 不能为空（至少登记一条命令）".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for (i, entry) in commands.iter().enumerate() {
            let manifest = manifest_for(entry).map_err(|e| format!("commands[{i}]: {e}"))?;
            if !seen.insert(manifest.name.clone()) {
                return Err(format!("commands[{i}]: tool 名 `{}` 重复", manifest.name));
            }
            let program = entry.get("program").and_then(|v| v.as_str()).unwrap_or("");
            if program.trim().is_empty() {
                return Err(format!("commands[{i}]: 缺少 program"));
            }
            if let Some(args) = entry.get("args") {
                if !args.is_array() || args.as_array().unwrap().iter().any(|a| !a.is_string()) {
                    return Err(format!("commands[{i}]: args 必须是字符串数组"));
                }
            }
            if let Some(stdin) = entry.get("stdin") {
                if !stdin.is_string() {
                    return Err(format!(
                        "commands[{i}]: stdin 必须是字符串（支持 {{参数}} 模板）"
                    ));
                }
            }
        }
        Ok(())
    }

    async fn list_tools(&self, instance: &SoftwareInstance) -> Result<Vec<ToolManifest>, String> {
        Ok(commands_of(&instance.config)
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
        let entry = commands_of(&instance.config)
            .into_iter()
            .find(|e| e.get("tool").and_then(|v| v.as_str()) == Some(tool))
            .ok_or_else(|| format!("工具不存在: {tool}"))?;

        let program = entry
            .get("program")
            .and_then(|v| v.as_str())
            .ok_or("缺少 program")?
            .to_string();
        let raw_args: Vec<String> = entry
            .get("args")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|a| a.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let rendered_args: Vec<String> = raw_args
            .into_iter()
            .map(|a| executor::render_template(&a, args))
            .collect();
        let stdin_text = entry
            .get("stdin")
            .and_then(|v| v.as_str())
            .map(|s| executor::render_template(s, args));
        let timeout = Duration::from_secs(
            entry
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .clamp(1, 300),
        );

        let outcome =
            executor::exec(&program, &rendered_args, stdin_text.as_deref(), timeout).await;
        let result = serde_json::json!({
            "success": outcome.success,
            "exit_code": outcome.exit_code,
            "timed_out": outcome.timed_out,
            "stdout": outcome.stdout,
            "stderr": outcome.stderr,
        });
        if outcome.success {
            Ok(result)
        } else {
            Err(serde_json::to_string(&result).unwrap_or_else(|_| outcome.stderr.clone()))
        }
    }
}

/// 参数名列表暴露给测试与桥接层使用。
pub fn declared_params(entry: &serde_json::Value) -> Vec<String> {
    param_names(entry.get("params").unwrap_or(&serde_json::Value::Null))
}
