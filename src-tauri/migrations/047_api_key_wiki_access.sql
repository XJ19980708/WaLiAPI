-- Wiki 查询权限独立于 RAG 知识库查询权限；旧密钥默认无 Wiki 权限。
-- 与 api_key_knowledge_access 平行：按 Wiki 项目粒度授权、可独立撤销。
CREATE TABLE api_key_wiki_access (
    api_key_id TEXT NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES wiki_projects(id) ON DELETE CASCADE,
    PRIMARY KEY (api_key_id, project_id)
);