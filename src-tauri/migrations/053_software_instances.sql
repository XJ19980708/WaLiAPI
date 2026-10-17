-- 软件对接能力域：本地软件实例注册表 + 工具调用审计日志
-- 软件实例 = 连接器（connector_id）+ 实例配置（config JSON）
-- mcp_enabled / enabled 双开关对齐 kb_knowledge_bases.mcp_enabled 模式
-- approval_mode: auto=自动放行 / confirm_high_risk=高危需确认 / confirm_all=全部需确认

CREATE TABLE IF NOT EXISTS software_instances (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    connector_id TEXT NOT NULL,
    name TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    mcp_enabled INTEGER NOT NULL DEFAULT 0,
    approval_mode TEXT NOT NULL DEFAULT 'confirm_high_risk',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_software_instances_connector ON software_instances(connector_id);

CREATE TABLE IF NOT EXISTS software_tool_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id INTEGER NOT NULL,
    instance_name TEXT NOT NULL DEFAULT '',
    tool TEXT NOT NULL,
    risk TEXT NOT NULL DEFAULT 'read_only',
    args_summary TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    error TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_software_tool_logs_instance ON software_tool_logs(instance_id, created_at);
