use super::models::*;
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

pub struct WikiRepository {
    pool: SqlitePool,
}

impl WikiRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    fn uuid() -> String {
        Uuid::new_v4().to_string()
    }

    // ── Project CRUD ──

    pub async fn list_projects(&self) -> Result<Vec<WikiProject>, String> {
        sqlx::query_as::<_, WikiProject>("SELECT * FROM wiki_projects ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn get_project(&self, id: &str) -> Result<WikiProject, String> {
        sqlx::query_as::<_, WikiProject>("SELECT * FROM wiki_projects WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn create_project(
        &self,
        input: &CreateProjectInput,
        wiki_dir: &str,
    ) -> Result<WikiProject, String> {
        let id = Self::uuid();
        self.create_project_with_id(&id, input, wiki_dir).await
    }

    pub async fn create_project_with_id(
        &self,
        id: &str,
        input: &CreateProjectInput,
        wiki_dir: &str,
    ) -> Result<WikiProject, String> {
        let now = Self::now();
        let schema = input
            .schema_text
            .clone()
            .unwrap_or_else(|| DEFAULT_SCHEMA.to_string());

        sqlx::query(
            "INSERT INTO wiki_projects (id, name, description, status, schema_text, wiki_dir, ingest_model, chat_model, ingest_channel_id, chat_channel_id, pdf_ocr_channel_id, pdf_ocr_model, reasoning_effort, temperature, mcp_enabled, source_count, page_count, created_at, updated_at)
             VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 0, 0, ?, ?)"
        )
        .bind(&id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&schema)
        .bind(wiki_dir)
        .bind(&input.ingest_model)
        .bind(&input.chat_model)
        .bind(&input.ingest_channel_id)
        .bind(&input.chat_channel_id)
        .bind(&input.pdf_ocr_channel_id)
        .bind(&input.pdf_ocr_model)
        .bind(&input.reasoning_effort)
        .bind(&input.temperature)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        // 与 RAG 平行：新建 Wiki 项目默认授权全部现有密钥。
        sqlx::query(
            "INSERT INTO api_key_wiki_access (api_key_id, project_id)
             SELECT id, ? FROM api_keys",
        )
        .bind(&id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        self.get_project(&id).await
    }

    pub async fn update_project(
        &self,
        id: &str,
        input: &UpdateProjectInput,
    ) -> Result<WikiProject, String> {
        let now = Self::now();
        let current = self.get_project(id).await?;

        let name = input.name.clone().unwrap_or(current.name);
        let description = input.description.clone().or(current.description);
        let status = input.status.unwrap_or(current.status);
        let schema_text = input.schema_text.clone().or(current.schema_text);
        let ingest_model = input.ingest_model.clone().or(current.ingest_model);
        let chat_model = input.chat_model.clone().or(current.chat_model);
        let ingest_channel_id = input
            .ingest_channel_id
            .clone()
            .or(current.ingest_channel_id);
        let chat_channel_id = input.chat_channel_id.clone().or(current.chat_channel_id);
        let pdf_ocr_channel_id = input
            .pdf_ocr_channel_id
            .clone()
            .or(current.pdf_ocr_channel_id);
        let pdf_ocr_model = input.pdf_ocr_model.clone().or(current.pdf_ocr_model);
        let reasoning_effort = input.reasoning_effort.clone().or(current.reasoning_effort);
        let temperature = input.temperature.or(current.temperature);
        let mcp_enabled = input.mcp_enabled.unwrap_or(current.mcp_enabled);

        sqlx::query(
            "UPDATE wiki_projects SET name=?, description=?, status=?, schema_text=?, ingest_model=?, chat_model=?, ingest_channel_id=?, chat_channel_id=?, pdf_ocr_channel_id=?, pdf_ocr_model=?, reasoning_effort=?, temperature=?, mcp_enabled=?, updated_at=? WHERE id=?"
        )
        .bind(&name)
        .bind(&description)
        .bind(status)
        .bind(&schema_text)
        .bind(&ingest_model)
        .bind(&chat_model)
        .bind(&ingest_channel_id)
        .bind(&chat_channel_id)
        .bind(&pdf_ocr_channel_id)
        .bind(&pdf_ocr_model)
        .bind(&reasoning_effort)
        .bind(temperature)
        .bind(mcp_enabled)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        self.get_project(id).await
    }

    pub async fn delete_project(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM wiki_projects WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    // ── Pages ──

    pub async fn list_pages(&self, project_id: &str) -> Result<Vec<WikiPage>, String> {
        sqlx::query_as::<_, WikiPage>(
            "SELECT * FROM wiki_pages WHERE project_id = ? AND status = 'active' ORDER BY path",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn get_page(&self, project_id: &str, path: &str) -> Result<Option<WikiPage>, String> {
        sqlx::query_as::<_, WikiPage>("SELECT * FROM wiki_pages WHERE project_id = ? AND path = ?")
            .bind(project_id)
            .bind(path)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn upsert_page(
        &self,
        project_id: &str,
        path: &str,
        title: &str,
        page_type: &str,
        content_hash: &str,
        token_count: i64,
        wikilinks: &str,
        frontmatter: &str,
        tags: &str,
    ) -> Result<(), String> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO wiki_pages (id, project_id, path, title, page_type, content_hash, token_count, wikilinks, frontmatter, tags, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)
             ON CONFLICT(project_id, path) DO UPDATE SET
               title=excluded.title, page_type=excluded.page_type,
               content_hash=excluded.content_hash, token_count=excluded.token_count,
               wikilinks=excluded.wikilinks, frontmatter=excluded.frontmatter,
               tags=excluded.tags, status='active', updated_at=excluded.updated_at"
        )
        .bind(Self::uuid())
        .bind(project_id)
        .bind(path)
        .bind(title)
        .bind(page_type)
        .bind(content_hash)
        .bind(token_count)
        .bind(wikilinks)
        .bind(frontmatter)
        .bind(tags)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    pub async fn delete_page(&self, project_id: &str, path: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM wiki_pages WHERE project_id = ? AND path = ?")
            .bind(project_id)
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    pub async fn search_pages(
        &self,
        project_id: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<WikiSearchResult>, String> {
        // 实测缺陷：原实现把整句 query 当连续子串匹配，页面叫「首次审核」时
        // 查「首次申请审核」必然 0 命中（中文无空格分词）。改为：整句优先 + 分词召回 + 命中数打分。
        let terms = query_terms(query);
        let pattern = format!("%{}%", query);

        // 1) 整句 LIKE（精确优先）
        let like_rows = sqlx::query_as::<_, WikiPage>(
            "SELECT * FROM wiki_pages WHERE project_id = ? AND status = 'active'
             AND (title LIKE ? OR path LIKE ?)
             ORDER BY title LIMIT ?",
        )
        .bind(project_id)
        .bind(&pattern)
        .bind(&pattern)
        .bind(top_k as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        let mut results: Vec<WikiSearchResult> = like_rows
            .iter()
            .map(|p| WikiSearchResult {
                page_id: p.id.clone(),
                path: p.path.clone(),
                title: p.title.clone(),
                score: 1.0,
                snippet: String::new(),
                page_type: p.page_type.clone(),
            })
            .collect();
        let mut seen: std::collections::HashSet<String> =
            results.iter().map(|r| r.path.clone()).collect();

        // 2) 分词 LIKE 召回（title/path 含任一词）
        if results.len() < top_k && !terms.is_empty() {
            let mut sql = String::from(
                "SELECT * FROM wiki_pages WHERE project_id = ? AND status = 'active' AND (",
            );
            for (i, _) in terms.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("title LIKE ? OR path LIKE ?");
            }
            sql.push_str(") ORDER BY title LIMIT ?");
            let mut q = sqlx::query_as::<_, WikiPage>(&sql).bind(project_id);
            let patterns: Vec<String> = terms.iter().map(|t| format!("%{}%", t)).collect();
            for p in &patterns {
                q = q.bind(p.as_str()).bind(p.as_str());
            }
            q = q.bind(top_k as i64 * 2);
            for p in q
                .fetch_all(&self.pool)
                .await
                .map_err(|e| format!("DB error: {}", e))?
            {
                if seen.insert(p.path.clone()) {
                    results.push(WikiSearchResult {
                        page_id: p.id.clone(),
                        path: p.path.clone(),
                        title: p.title.clone(),
                        score: 0.9,
                        snippet: String::new(),
                        page_type: p.page_type.clone(),
                    });
                }
            }
        }

        // 3) 读盘按“命中词数”打分召回（正文含关键词但标题没有的页面）
        if results.len() < top_k {
            let all_pages = sqlx::query_as::<_, WikiPage>(
                "SELECT * FROM wiki_pages WHERE project_id = ? AND status = 'active' ORDER BY title"
            )
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))?;

            for page in &all_pages {
                if seen.contains(&page.path) {
                    continue;
                }
                if let Ok(content) =
                    crate::services::wiki::project::read_page(project_id, &page.path).await
                {
                    let hit_terms: Vec<&String> = terms
                        .iter()
                        .filter(|t| find_case_insensitive_match_start(&content, t).is_some())
                        .collect();
                    let whole_hit = find_case_insensitive_match_start(&content, query).is_some();
                    if hit_terms.is_empty() && !whole_hit {
                        continue;
                    }
                    // snippet：优先整句上下文，否则取首个命中词上下文
                    let snippet = if whole_hit {
                        build_search_snippet(&content, query)
                    } else {
                        hit_terms
                            .iter()
                            .find_map(|t| build_search_snippet(&content, t))
                    }
                    .unwrap_or_else(|| content.chars().take(200).collect::<String>());
                    let score = if whole_hit {
                        0.95
                    } else {
                        0.5 + (hit_terms.len().min(6) as f64) * 0.08
                    };
                    results.push(WikiSearchResult {
                        page_id: page.id.clone(),
                        path: page.path.clone(),
                        title: page.title.clone(),
                        score,
                        snippet,
                        page_type: page.page_type.clone(),
                    });
                }
            }
            // 命中越多越靠前
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            results.truncate(top_k);
        } else {
            // Add snippets for LIKE matches too
            for r in &mut results {
                if let Ok(content) =
                    crate::services::wiki::project::read_page(project_id, &r.path).await
                {
                    if let Some(snippet) = build_search_snippet(&content, query) {
                        r.snippet = snippet;
                    } else {
                        // Use first 200 chars as snippet
                        r.snippet = content.chars().take(200).collect::<String>();
                    }
                }
            }
        }

        Ok(results)
    }

    // ── Sources ──

    pub async fn list_sources(&self, project_id: &str) -> Result<Vec<WikiSource>, String> {
        sqlx::query_as::<_, WikiSource>(
            "SELECT * FROM wiki_sources WHERE project_id = ? ORDER BY created_at DESC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn add_source(
        &self,
        project_id: &str,
        input: &AddSourceInput,
        content_hash: Option<&str>,
        file_size: i64,
    ) -> Result<WikiSource, String> {
        let id = Self::uuid();
        let now = Self::now();
        sqlx::query(
            "INSERT INTO wiki_sources (id, project_id, source_type, filename, file_path, source_url, content_hash, file_size, status, page_count, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', 0, ?)"
        )
        .bind(&id)
        .bind(project_id)
        .bind(&input.source_type)
        .bind(&input.filename)
        .bind(&input.file_path)
        .bind(&input.source_url)
        .bind(content_hash)
        .bind(file_size)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        sqlx::query_as::<_, WikiSource>("SELECT * FROM wiki_sources WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn delete_source(&self, source_id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM wiki_sources WHERE id = ?")
            .bind(source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    pub async fn update_source_status(
        &self,
        source_id: &str,
        status: &str,
        page_count: i64,
        error: Option<&str>,
    ) -> Result<(), String> {
        let now = Self::now();
        sqlx::query(
            "UPDATE wiki_sources SET status=?, page_count=?, error_message=?, ingested_at=? WHERE id=?"
        )
        .bind(status)
        .bind(page_count)
        .bind(error)
        .bind(if status == "ingested" { Some(now.clone()) } else { None })
        .bind(source_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    // ── Ingest Queue ──

    pub async fn create_task(
        &self,
        project_id: &str,
        source_id: Option<&str>,
        task_type: &str,
    ) -> Result<String, String> {
        let id = Self::uuid();
        let now = Self::now();
        sqlx::query(
            "INSERT INTO wiki_ingest_queue (id, project_id, source_id, task_type, status, progress, total_steps, done_steps, created_at)
             VALUES (?, ?, ?, ?, 'pending', 0, 0, 0, ?)"
        )
        .bind(&id)
        .bind(project_id)
        .bind(source_id)
        .bind(task_type)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
        Ok(id)
    }

    /// 取消某来源的进行中摄入任务（queue status 置 cancelled）。
    /// 摄入循环在 await 边界检查到后即终止；已完成/失败的任务不触碰。
    pub async fn cancel_source_ingest(&self, source_id: &str) -> Result<(), String> {
        sqlx::query(
            "UPDATE wiki_ingest_queue SET status='cancelled', completed_at=? \
             WHERE source_id=? AND status IN ('running','pending')",
        )
        .bind(Self::now())
        .bind(source_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    pub async fn update_task_status(
        &self,
        task_id: &str,
        status: &str,
        progress: i64,
        done_steps: i64,
        total_steps: i64,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let now = Self::now();
        let started = if status == "running" {
            Some(now.clone())
        } else {
            None
        };
        let completed = if status == "done" || status == "failed" {
            Some(now.clone())
        } else {
            None
        };
        sqlx::query(
            "UPDATE wiki_ingest_queue SET status=?, progress=?, done_steps=?, total_steps=?, result_json=?, error_message=?, started_at=COALESCE(?, started_at), completed_at=COALESCE(?, completed_at) WHERE id=?"
        )
        .bind(status)
        .bind(progress)
        .bind(done_steps)
        .bind(total_steps)
        .bind(result)
        .bind(error)
        .bind(started.as_deref())
        .bind(completed.as_deref())
        .bind(task_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    pub async fn list_tasks(&self, project_id: &str) -> Result<Vec<WikiIngestTask>, String> {
        sqlx::query_as::<_, WikiIngestTask>(
            "SELECT * FROM wiki_ingest_queue WHERE project_id = ? ORDER BY created_at DESC LIMIT 20"
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))
    }

    // ── Sessions ──

    pub async fn list_sessions(&self, project_id: &str) -> Result<Vec<WikiSession>, String> {
        sqlx::query_as::<_, WikiSession>(
            "SELECT * FROM wiki_sessions WHERE project_id = ? ORDER BY created_at DESC LIMIT 50",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))
    }

    pub async fn add_session(
        &self,
        project_id: &str,
        role: &str,
        content: &str,
        sources: Option<&str>,
        model: Option<&str>,
    ) -> Result<(), String> {
        let id = Self::uuid();
        let now = Self::now();
        sqlx::query(
            "INSERT INTO wiki_sessions (id, project_id, role, content, sources_json, model, tokens_used, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 0, ?)"
        )
        .bind(&id)
        .bind(project_id)
        .bind(role)
        .bind(content)
        .bind(sources)
        .bind(model)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    pub async fn clear_sessions(&self, project_id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM wiki_sessions WHERE project_id = ?")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("DB error: {}", e))?;
        Ok(())
    }

    // ── Tags ──

    pub async fn get_tags(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::services::wiki::models::WikiTag>, String> {
        // Collect tags from all active pages in the project
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT tags FROM wiki_pages WHERE project_id = ? AND status = 'active'",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (tags_json,) in &rows {
            let tags: Vec<String> = serde_json::from_str(tags_json).unwrap_or_default();
            for tag in tags {
                let tag = tag.trim().to_lowercase();
                if !tag.is_empty() {
                    *freq.entry(tag).or_insert(0) += 1;
                }
            }
        }

        let mut freq_vec: Vec<(String, usize)> = freq.into_iter().collect();
        freq_vec.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(freq_vec
            .into_iter()
            .take(limit)
            .map(|(word, count)| crate::services::wiki::models::WikiTag { word, count })
            .collect())
    }

    // ── Graph ──

    pub async fn get_graph(&self, project_id: &str) -> Result<GraphData, String> {
        let pages = self.list_pages(project_id).await?;

        let nodes: Vec<GraphNode> = pages
            .iter()
            .map(|p| {
                let links: Vec<String> = serde_json::from_str(&p.wikilinks).unwrap_or_default();
                GraphNode {
                    id: p.path.clone(),
                    label: p.title.clone(),
                    path: Some(p.path.clone()),
                    node_type: p.page_type.clone(),
                    link_count: links.len(),
                }
            })
            .collect();

        let edges_rows = sqlx::query_as::<_, WikiGraphEdgeRow>(
            "SELECT * FROM wiki_graph_edges WHERE project_id = ? ORDER BY weight DESC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        let edges: Vec<GraphEdge> = edges_rows
            .iter()
            .map(|r| GraphEdge {
                source: r.source_page.clone(),
                target: r.target_page.clone(),
                edge_type: r.edge_type.clone(),
                weight: r.weight,
            })
            .collect();

        Ok(GraphData { nodes, edges })
    }

    // ── Stats ──

    pub async fn get_stats(&self, project_id: &str) -> Result<serde_json::Value, String> {
        let page_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wiki_pages WHERE project_id = ? AND status = 'active'",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        let source_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wiki_sources WHERE project_id = ? AND status = 'ingested'",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        let review_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wiki_reviews WHERE project_id = ? AND resolved = 0",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        let page_types: Vec<(String, i64)> = sqlx::query_as(
            "SELECT page_type, COUNT(*) as cnt FROM wiki_pages WHERE project_id = ? AND status = 'active' GROUP BY page_type"
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let type_map: serde_json::Map<String, serde_json::Value> = page_types
            .into_iter()
            .map(|(t, c)| (t, serde_json::Value::Number(c.into())))
            .collect();

        Ok(serde_json::json!({
            "pages": page_count,
            "sources": source_count,
            "pending_reviews": review_count,
            "page_types": serde_json::Value::Object(type_map),
        }))
    }
}

/// 是否 CJK 表意文字（覆盖常用中文区）。
fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x3040..=0x30FF
    )
}

/// 把自然语言查询切成检索词：
/// - ASCII 字母数字段整段成词（小写）
/// - CJK 连续串按 **2-gram 滑窗**切词（中文无空格，"首次申请审核" → 首次/次申/申请/请审/审核）
/// 用于修复"整句子串匹配"导致中文长问句 0 命中的缺陷。
fn query_terms(query: &str) -> Vec<String> {
    let chars: Vec<char> = query.chars().collect();
    let mut terms: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_cjk(c) {
            let start = i;
            while i < chars.len() && is_cjk(chars[i]) {
                i += 1;
            }
            let seg: Vec<char> = chars[start..i].to_vec();
            if seg.len() <= 2 {
                terms.push(seg.iter().collect());
            } else {
                for w in seg.windows(2) {
                    terms.push(w.iter().collect());
                }
            }
        } else if c.is_alphanumeric() {
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() && !is_cjk(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            terms.push(word.to_lowercase());
        } else {
            i += 1;
        }
    }
    terms.retain(|t| !t.trim().is_empty());
    terms.dedup();
    // 控制 SQL 规模与噪声
    terms.truncate(24);
    terms
}

fn build_search_snippet(content: &str, query: &str) -> Option<String> {
    let match_start = find_case_insensitive_match_start(content, query)?;
    let match_char_index = content[..match_start].chars().count();
    let snippet_start = match_char_index.saturating_sub(60);
    let snippet = content
        .chars()
        .skip(snippet_start)
        .take(200)
        .collect::<String>()
        .replace('\n', " ");
    Some(format!("...{}...", snippet))
}

fn find_case_insensitive_match_start(content: &str, query: &str) -> Option<usize> {
    let query_lower = query.to_lowercase();
    let mut content_lower = String::with_capacity(content.len());
    let mut offsets = Vec::with_capacity(content.chars().count() + 1);

    for (original_offset, ch) in content.char_indices() {
        offsets.push((content_lower.len(), original_offset));
        content_lower.extend(ch.to_lowercase());
    }
    offsets.push((content_lower.len(), content.len()));

    let match_offset = content_lower.find(&query_lower)?;
    let offset_index = offsets
        .partition_point(|(lower_offset, _)| *lower_offset <= match_offset)
        .saturating_sub(1);
    Some(offsets[offset_index].1)
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct WikiGraphEdgeRow {
    id: String,
    project_id: String,
    source_page: String,
    target_page: String,
    edge_type: String,
    weight: f64,
    created_at: String,
}

/// 默认 Wiki Schema：编译时读取项目根目录 `wiki-prompt.txt` 的内容。
/// 该文件是用户维护的单一事实来源，修改后重新编译即生效。
pub const DEFAULT_SCHEMA: &str = include_str!("../../../../wiki-prompt.txt");

#[cfg(test)]
mod tests {
    use super::*;

    /// 实测缺陷回归：页面叫「首次审核」，用户问「首次申请审核需要哪些截图」，
    /// 整句子串匹配 0 命中；分词后必须切出 首次/申请/审核 等词用于召回。
    #[test]
    fn query_terms_splits_cjk_long_question_into_bigrams() {
        let terms = query_terms("首次申请审核需要哪些截图");
        for want in ["首次", "申请", "审核", "需要", "截图"] {
            assert!(terms.iter().any(|t| t == want), "缺少词 {want}: {terms:?}");
        }
        assert!(terms.len() <= 24);
        // 无分隔符的 ASCII 词整段成词并小写
        let mixed = query_terms("登录 PDF 手册");
        assert!(mixed.iter().any(|t| t == "pdf"), "{mixed:?}");
        assert!(mixed.iter().any(|t| t == "登录"), "{mixed:?}");
        // 空/标点查询不 panic
        assert!(query_terms("  ？？  ").is_empty() || query_terms("？？").len() <= 24);
    }

    #[test]
    fn search_snippet_handles_cjk_without_splitting_utf8() {
        let content = format!("{}阻止输出或报警{}", "前".repeat(80), "后".repeat(180));
        let snippet = build_search_snippet(&content, "输出").unwrap();

        assert!(snippet.contains("阻止输出或报警"));
        assert!(snippet.is_char_boundary(snippet.len()));
    }

    #[test]
    fn search_snippet_maps_unicode_lowercase_expansion_to_original_text() {
        let content = format!("{}İSTANBUL{}", "前".repeat(80), "后".repeat(180));
        let snippet = build_search_snippet(&content, "i\u{307}stanbul").unwrap();

        assert!(snippet.contains("İSTANBUL"));
        assert!(snippet.is_char_boundary(snippet.len()));
    }

    #[test]
    fn search_snippet_is_case_insensitive_for_ascii() {
        let snippet = build_search_snippet("WaLiAPI Wiki Gateway", "wiki").unwrap();
        assert!(snippet.contains("WaLiAPI Wiki Gateway"));
    }
}
