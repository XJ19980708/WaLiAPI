//! 软件实例模型与审批策略。

use serde::{Deserialize, Serialize};

/// 软件实例：一个连接器 + 一份用户配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftwareInstance {
    pub id: i64,
    pub connector_id: String,
    pub name: String,
    /// 连接器自定义配置（各连接器自行解析）
    pub config: serde_json::Value,
    /// 1 = 在 /mcp tools/list 中暴露该实例工具
    pub mcp_enabled: i64,
    /// auto / confirm_high_risk / confirm_all
    pub approval_mode: String,
    /// 1 = 实例启用
    pub enabled: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 审批模式。
pub const APPROVAL_AUTO: &str = "auto";
pub const APPROVAL_CONFIRM_HIGH_RISK: &str = "confirm_high_risk";
pub const APPROVAL_CONFIRM_ALL: &str = "confirm_all";

pub fn valid_approval_mode(mode: &str) -> bool {
    matches!(
        mode,
        APPROVAL_AUTO | APPROVAL_CONFIRM_HIGH_RISK | APPROVAL_CONFIRM_ALL
    )
}

/// 给定风险级别与审批模式，判断是否需要用户确认。
pub fn needs_confirmation(approval_mode: &str, risk: connector::RiskLevel) -> bool {
    use connector::RiskLevel;
    match risk {
        RiskLevel::ReadOnly => false,
        RiskLevel::LowWrite => approval_mode == APPROVAL_CONFIRM_ALL,
        RiskLevel::HighWrite => approval_mode != APPROVAL_AUTO,
    }
}

// connector 类型在同一模块树下直接引用
use super::connector;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_matrix() {
        assert!(!needs_confirmation(
            APPROVAL_AUTO,
            connector::RiskLevel::ReadOnly
        ));
        assert!(!needs_confirmation(
            APPROVAL_AUTO,
            connector::RiskLevel::LowWrite
        ));
        assert!(!needs_confirmation(
            APPROVAL_AUTO,
            connector::RiskLevel::HighWrite
        ));
        assert!(!needs_confirmation(
            APPROVAL_CONFIRM_HIGH_RISK,
            connector::RiskLevel::ReadOnly
        ));
        assert!(!needs_confirmation(
            APPROVAL_CONFIRM_HIGH_RISK,
            connector::RiskLevel::LowWrite
        ));
        assert!(needs_confirmation(
            APPROVAL_CONFIRM_HIGH_RISK,
            connector::RiskLevel::HighWrite
        ));
        assert!(needs_confirmation(
            APPROVAL_CONFIRM_ALL,
            connector::RiskLevel::LowWrite
        ));
        assert!(needs_confirmation(
            APPROVAL_CONFIRM_ALL,
            connector::RiskLevel::HighWrite
        ));
    }
}
