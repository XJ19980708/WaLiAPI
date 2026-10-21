//! `SoftwareConnector` trait 与工具描述类型。

use super::model::SoftwareInstance;
use async_trait::async_trait;

/// 工具风险级别（决定审批策略）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// 只读：不改变软件/系统状态
    ReadOnly,
    /// 低危写：普通写操作（新增笔记、加入队列等）
    LowWrite,
    /// 高危写：对外发送、删除、支付类等不可逆/影响他人的操作
    HighWrite,
}

impl RiskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::ReadOnly => "read_only",
            RiskLevel::LowWrite => "low_write",
            RiskLevel::HighWrite => "high_write",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            RiskLevel::ReadOnly => "只读",
            RiskLevel::LowWrite => "低危写",
            RiskLevel::HighWrite => "高危写",
        }
    }

    pub fn parse(s: &str) -> RiskLevel {
        match s {
            "low_write" => RiskLevel::LowWrite,
            "high_write" => RiskLevel::HighWrite,
            _ => RiskLevel::ReadOnly,
        }
    }
}

/// 连接器对外的一个工具（进 /mcp tools/list）。
#[derive(Debug, Clone)]
pub struct ToolManifest {
    /// 实例内短名（如 `list_chats`），MCP 全名由 mcp_bridge 组装命名空间
    pub name: String,
    pub description: String,
    /// JSON Schema（inputSchema）
    pub input_schema: serde_json::Value,
    pub risk: RiskLevel,
}

impl ToolManifest {
    pub fn read_only(name: &str, description: &str, input_schema: serde_json::Value) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            input_schema,
            risk: RiskLevel::ReadOnly,
        }
    }
}

/// 软件连接器：一个可对接的软件 / 通用对接方案。
#[async_trait]
pub trait SoftwareConnector: Send + Sync {
    /// 连接器唯一 id（入库 connector_id）
    fn id(&self) -> &'static str;
    /// 展示名
    fn label(&self) -> &'static str;
    /// 一句话说明（连接器市场卡片）
    fn description(&self) -> &'static str;
    /// 平台支持说明（macOS / Windows / 双端）
    fn platforms(&self) -> &'static str;
    /// 配置说明 + 示例（前端表单渲染与「插入示例」）
    fn sample_config(&self) -> serde_json::Value;
    /// 校验配置合法性（保存前调用）
    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String>;
    /// 列出该实例暴露的工具（每次 tools/list 动态调用）
    async fn list_tools(&self, instance: &SoftwareInstance) -> Result<Vec<ToolManifest>, String>;
    /// 执行工具调用（审批通过后才会进入）
    async fn call_tool(
        &self,
        instance: &SoftwareInstance,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}
