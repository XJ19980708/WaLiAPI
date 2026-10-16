-- 可观测性增强（P1）：
-- 1) request_logs 增加首字耗时 / 尝试次数 / 估算速度三列；
-- 2) log_prompt_parts：按消息分类（系统提示词/工具定义/注入内容/助手消息/工具结果/用户消息）
--    的 token 拆分，一行一个分类，对应前端「上下文构成」卡片；
-- 3) log_attempts：每次上游物理尝试的明细（渠道、状态、失败原因、耗时、TTFT），
--    对应前端「尝试时间线」与路由图 failover 回放。
-- 历史行三列均为 NULL/0，前端按 "-" 展示。

ALTER TABLE request_logs ADD COLUMN ttft_ms INTEGER;
ALTER TABLE request_logs ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;

CREATE TABLE log_prompt_parts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    log_id TEXT NOT NULL,
    kind TEXT NOT NULL,            -- system|tools|injected|assistant|tool_result|chat
    items INTEGER NOT NULL DEFAULT 0,
    tokens INTEGER NOT NULL DEFAULT 0,   -- 校准后（无真实用量时 = 估算值）
    est_tokens INTEGER NOT NULL DEFAULT 0, -- 纯本地估算值
    created_at TEXT NOT NULL
);
CREATE INDEX idx_log_prompt_parts_log ON log_prompt_parts(log_id);

CREATE TABLE log_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    log_id TEXT NOT NULL,
    seq INTEGER NOT NULL,          -- 尝试序号（1-based）
    channel_id TEXT,
    channel_name TEXT,
    upstream_model TEXT,
    status_code INTEGER,           -- 尚未收到响应时为 NULL
    outcome TEXT,                  -- success|retryable|caller_terminal|...（failure_kind）
    error_message TEXT,
    duration_ms INTEGER,           -- 该次尝试耗时
    ttft_ms INTEGER,               -- 流式首帧耗时（成功尝试才有意义）
    is_final INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_log_attempts_log ON log_attempts(log_id);
