-- Wiki 项目与 RAG 知识库：推理档位 + 温度配置
-- reasoning_effort: default/none/low/medium/high（NULL = 跟随模型默认）
-- temperature: 0~2（NULL = 用各入口默认值）

ALTER TABLE wiki_projects ADD COLUMN reasoning_effort TEXT;
ALTER TABLE wiki_projects ADD COLUMN temperature REAL;

ALTER TABLE kb_knowledge_bases ADD COLUMN reasoning_effort TEXT;
ALTER TABLE kb_knowledge_bases ADD COLUMN temperature REAL;