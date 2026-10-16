-- Wiki 项目与 RAG 知识库：推理档位 + 温度配置
-- reasoning_effort: default/none/low/medium/high（NULL = 跟随模型默认）
-- temperature: 0~2（NULL = 用各入口默认值）
--
-- 0.4.2 升级兼容：旧编号 049_wiki_kb_reasoning_temperature 重排为 052。旧库的 049 记录
-- 由启动时 db::backfill_reranked_migrations 映射注册为 052（视为已应用跳过），因此这里
-- 不会对已建列的升级库重复执行；新库正常执行建列。

ALTER TABLE wiki_projects ADD COLUMN reasoning_effort TEXT;
ALTER TABLE wiki_projects ADD COLUMN temperature REAL;

ALTER TABLE kb_knowledge_bases ADD COLUMN reasoning_effort TEXT;
ALTER TABLE kb_knowledge_bases ADD COLUMN temperature REAL;
