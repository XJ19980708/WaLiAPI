//! CLI-Hub 连接器：包装 [CLI-Anything](https://github.com/HKUDS/CLI-Anything)
//! 社区生成的 agent 原生 CLI（`cli-anything-<software>`）。
//!
//! 设计取舍：CLI-Anything 的 CLI 本身已为 agent 设计（子命令分组、
//! 自描述 `--help`、统一 `--json` 输出），因此不为每个子命令硬编码
//! 工具，而是暴露两个稳定工具：
//! - `help`（read_only）：探索命令面（`<binary> [--json] <子命令...> --help`）
//! - `run`（风险可配，默认 low_write）：执行子命令（自动前置 `--json`）
//!
//! 安全约定与 generic_cli 一致：子进程直接 exec 不经 shell；
//! `command` 字符串按空白切分为参数数组，不支持引号包裹（文档声明）。

use super::super::connector::{RiskLevel, ToolManifest};
use super::super::executor;
use super::super::model::SoftwareInstance;
use crate::services::software::connector::SoftwareConnector;
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

pub struct ClihubConnector;

/// 按空白切分命令串为参数数组（不支持引号包裹，见模块注释）。
fn split_command(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn cli_binary(config: &serde_json::Value) -> Result<String, String> {
    config
        .get("binary")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("config.binary 缺失（CLI-Anything 入口命令，如 cli-anything-gimp）".to_string())
}

fn intro_of(config: &serde_json::Value) -> String {
    config
        .get("intro")
        .and_then(|v| v.as_str())
        .unwrap_or("CLI-Anything 生成的 agent 原生 CLI")
        .to_string()
}

fn run_risk(config: &serde_json::Value) -> RiskLevel {
    RiskLevel::parse(
        config
            .get("run_risk")
            .and_then(|v| v.as_str())
            .unwrap_or("low_write"),
    )
}

/// 执行 `<binary> <args...>` 并包装为统一结果。
async fn run_binary(
    binary: &str,
    args: &[String],
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let outcome = executor::exec(binary, args, None, timeout);
    let outcome = outcome.await;
    let mut result = json!({
        "success": outcome.success,
        "exit_code": outcome.exit_code,
        "timed_out": outcome.timed_out,
        "stdout": outcome.stdout,
        "stderr": outcome.stderr,
    });
    // run 的 stdout 通常为 --json 输出，尝试解析后内嵌，方便 agent 直接消费
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(outcome.stdout.trim()) {
        result["json"] = parsed;
    }
    if outcome.success {
        Ok(result)
    } else {
        Err(serde_json::to_string(&result).unwrap_or_else(|_| outcome.stderr.clone()))
    }
}

#[async_trait]
impl SoftwareConnector for ClihubConnector {
    fn id(&self) -> &'static str {
        "clihub"
    }
    fn label(&self) -> &'static str {
        "CLI-Hub 社区 CLI"
    }
    fn description(&self) -> &'static str {
        "包装 CLI-Anything 社区生成的 agent 原生 CLI（cli-anything-*）：help 探索命令面 + run 以 --json 执行子命令，先装 cli-anything-hub 再一键接入"
    }
    fn platforms(&self) -> &'static str {
        "macOS / Windows / Linux（需 Python 3.10+ 与 pip 安装的 CLI）"
    }
    fn sample_config(&self) -> serde_json::Value {
        json!({
            "cli": "gimp",
            "binary": "cli-anything-gimp",
            "intro": "GIMP 图像编辑 agent 原生 CLI",
            "run_risk": "low_write"
        })
    }

    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String> {
        let binary = cli_binary(config)?;
        if !super::valid_tool_name(&binary.replace('-', "_")) {
            return Err(format!("非法 binary 名 `{binary}`"));
        }
        let risk = config
            .get("run_risk")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !risk.is_empty() && !["read_only", "low_write", "high_write"].contains(&risk) {
            return Err(format!(
                "非法 run_risk `{risk}`（read_only / low_write / high_write）"
            ));
        }
        Ok(())
    }

    async fn list_tools(&self, instance: &SoftwareInstance) -> Result<Vec<ToolManifest>, String> {
        let binary = cli_binary(&instance.config)?;
        let intro = intro_of(&instance.config);
        Ok(vec![
            ToolManifest {
                name: "help".into(),
                description: format!(
                    "{intro} —— 探索命令面。不传 subcommand 返回顶层用法；传子命令路径（空格分隔，如 `project create`）返回该子命令的详细帮助。执行 run 前先用本工具确认用法。"
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "subcommand": {
                            "type": "string",
                            "description": "可选。子命令路径，空格分隔（如 project create）"
                        }
                    },
                    "additionalProperties": false
                }),
                risk: RiskLevel::ReadOnly,
            },
            ToolManifest {
                name: "run".into(),
                description: format!(
                    "{intro} —— 执行子命令（自动追加 --json，返回结构化结果）。命令串按空格切分、不支持引号包裹；先调 help 确认子命令与参数再执行。"
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "子命令与参数，空格分隔（如 project create --name demo）"
                        }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
                risk: run_risk(&instance.config),
            }],
        )
        .map(|tools| {
            let _ = binary; // binary 仅用于校验存在性（见 call_tool）
            tools
        })
    }

    async fn call_tool(
        &self,
        instance: &SoftwareInstance,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let binary = cli_binary(&instance.config)?;
        match tool {
            "help" => {
                let sub = args
                    .get("subcommand")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                let mut cli_args: Vec<String> = split_command(sub).into_iter().collect();
                cli_args.push("--help".to_string());
                run_binary(&binary, &cli_args, Duration::from_secs(30)).await
            }
            "run" => {
                let command = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 command 参数")?
                    .trim();
                if command.is_empty() {
                    return Err("command 不能为空".to_string());
                }
                let mut cli_args = vec!["--json".to_string()];
                cli_args.extend(split_command(command));
                let timeout = Duration::from_secs(
                    instance
                        .config
                        .get("timeout_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(120)
                        .clamp(5, 600),
                );
                run_binary(&binary, &cli_args, timeout).await
            }
            _ => Err(format!("工具不存在: {tool}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::software::model::SoftwareInstance;

    fn instance(config: serde_json::Value) -> SoftwareInstance {
        SoftwareInstance {
            id: 1,
            connector_id: "clihub".into(),
            name: "t".into(),
            config,
            mcp_enabled: 1,
            enabled: 1,
            approval_mode: "confirm_high_risk".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn split_command_by_whitespace() {
        assert_eq!(
            split_command("project create --name demo"),
            vec!["project", "create", "--name", "demo"]
        );
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn validate_requires_binary() {
        let c = ClihubConnector;
        assert!(c
            .validate_config(&json!({"binary": "cli-anything-gimp"}))
            .is_ok());
        assert!(c.validate_config(&json!({})).is_err());
        assert!(c
            .validate_config(&json!({"binary": "x", "run_risk": "bogus"}))
            .is_err());
    }

    #[tokio::test]
    async fn list_tools_exposes_help_and_run() {
        let c = ClihubConnector;
        let inst = instance(json!({"binary": "cli-anything-gimp", "intro": "GIMP CLI"}));
        let tools = c.list_tools(&inst).await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "help");
        assert_eq!(tools[0].risk, RiskLevel::ReadOnly);
        assert_eq!(tools[1].name, "run");
        assert_eq!(tools[1].risk, RiskLevel::LowWrite);
        assert!(tools[0].description.contains("GIMP CLI"));
    }

    #[tokio::test]
    async fn help_calls_binary_with_json_flag() {
        let c = ClihubConnector;
        let inst = instance(json!({"binary": "echo"}));
        let out = c
            .call_tool(&inst, "help", &json!({"subcommand": "project create"}))
            .await
            .unwrap();
        // echo 会把参数原样吐回来，借此断言参数拼装
        assert!(out["stdout"].as_str().unwrap().contains("--help"));
        assert!(out["stdout"].as_str().unwrap().contains("project create"));
    }

    #[tokio::test]
    async fn run_prepends_json_flag_and_parses_output() {
        let c = ClihubConnector;
        let inst = instance(json!({"binary": "echo"}));
        let out = c
            .call_tool(&inst, "run", &json!({"command": "status list"}))
            .await
            .unwrap();
        assert!(out["stdout"].as_str().unwrap().starts_with("--json"));
        assert!(out["stdout"].as_str().unwrap().contains("status list"));
    }

    #[tokio::test]
    async fn run_rejects_empty_command() {
        let c = ClihubConnector;
        let inst = instance(json!({"binary": "echo"}));
        assert!(c.call_tool(&inst, "run", &json!({})).await.is_err());
        assert!(c
            .call_tool(&inst, "run", &json!({"command": "  "}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn unknown_tool_rejected() {
        let c = ClihubConnector;
        let inst = instance(json!({"binary": "echo"}));
        assert!(c.call_tool(&inst, "nope", &json!({})).await.is_err());
    }
}
