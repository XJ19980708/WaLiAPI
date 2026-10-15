-- 调用来源（client attribution）：记录请求来自哪个下游接入软件。
-- 值为从 User-Agent 等请求头识别出的友好名称（如 "Claude Code"、"WorkBuddy"），
-- 识别不出时为 NULL。历史行为 NULL，前端展示为 "-"。
ALTER TABLE request_logs ADD COLUMN client_name TEXT;
