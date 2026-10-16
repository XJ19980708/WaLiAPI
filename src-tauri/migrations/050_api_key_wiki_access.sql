-- Wiki 查询权限独立于 RAG 知识库查询权限；旧密钥默认无 Wiki 权限。
-- 与 api_key_knowledge_access 平行：按 Wiki 项目粒度授权、可独立撤销。
-- 幂等重建：合并修复前的历史库可能残留旧列名（wiki_project_id）的空表，
-- DROP 后按代码使用的列名（project_id）重建，避免列名漂移导致运行时报错。
DROP TABLE IF EXISTS api_key_wiki_access;
CREATE TABLE api_key_wiki_access (
    api_key_id TEXT NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES wiki_projects(id) ON DELETE CASCADE,
    PRIMARY KEY (api_key_id, project_id)
);
