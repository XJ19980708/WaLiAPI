//! software_instances / software_tool_logs 仓储。

use super::model::SoftwareInstance;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct SoftwareRepository {
    pool: SqlitePool,
}

/// software_instances 查询行（列顺序见 SELECT）。
type InstanceRow = (
    i64,
    String,
    String,
    String,
    i64,
    String,
    i64,
    String,
    String,
);

/// software_tool_logs 查询行（列顺序见 SELECT）。
type ToolLogRow = (i64, String, String, String, String, String, i64, String);

const INSTANCE_COLUMNS: &str =
    "id, connector_id, name, config, mcp_enabled, approval_mode, enabled, created_at, updated_at";

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 数据行 → 实例（config 列为 JSON 字符串，解析失败回退 `{}`）。
fn row_to_instance(row: InstanceRow) -> SoftwareInstance {
    SoftwareInstance {
        id: row.0,
        connector_id: row.1,
        name: row.2,
        config: serde_json::from_str(&row.3).unwrap_or(serde_json::json!({})),
        mcp_enabled: row.4,
        approval_mode: row.5,
        enabled: row.6,
        created_at: row.7,
        updated_at: row.8,
    }
}

/// 一次工具调用的审计信息。
pub struct ToolLogEntry {
    pub tool: String,
    pub risk: String,
    pub args_summary: String,
    pub status: String,
    pub error: String,
    pub duration_ms: i64,
}

impl SoftwareRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get_all(&self) -> Result<Vec<SoftwareInstance>, String> {
        let sql = format!("SELECT {INSTANCE_COLUMNS} FROM software_instances ORDER BY id");
        let rows: Vec<InstanceRow> = sqlx::query_as(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(rows.into_iter().map(row_to_instance).collect())
    }

    pub async fn get(&self, id: i64) -> Result<SoftwareInstance, String> {
        let sql = format!("SELECT {INSTANCE_COLUMNS} FROM software_instances WHERE id = ?");
        let row: InstanceRow = sqlx::query_as(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("软件实例不存在: {id}"))?;
        Ok(row_to_instance(row))
    }

    /// 已启用且已开启 MCP 的实例（/mcp 暴露范围）。
    pub async fn list_exposed(&self) -> Result<Vec<SoftwareInstance>, String> {
        Ok(self
            .get_all()
            .await?
            .into_iter()
            .filter(|i| i.enabled == 1 && i.mcp_enabled == 1)
            .collect())
    }

    pub async fn create(
        &self,
        connector_id: &str,
        name: &str,
        config: &serde_json::Value,
        mcp_enabled: bool,
        approval_mode: &str,
    ) -> Result<SoftwareInstance, String> {
        let config_str = serde_json::to_string(config).map_err(|e| e.to_string())?;
        let ts = now();
        let result = sqlx::query(
            "INSERT INTO software_instances (connector_id, name, config, mcp_enabled, approval_mode, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(connector_id)
        .bind(name)
        .bind(&config_str)
        .bind(mcp_enabled as i64)
        .bind(approval_mode)
        .bind(&ts)
        .bind(&ts)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        self.get(result.last_insert_rowid()).await
    }

    pub async fn update(
        &self,
        id: i64,
        name: Option<&str>,
        config: Option<&serde_json::Value>,
        mcp_enabled: Option<bool>,
        approval_mode: Option<&str>,
    ) -> Result<SoftwareInstance, String> {
        if let Some(n) = name {
            sqlx::query("UPDATE software_instances SET name = ?, updated_at = ? WHERE id = ?")
                .bind(n)
                .bind(now())
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        }
        if let Some(c) = config {
            let config_str = serde_json::to_string(c).map_err(|e| e.to_string())?;
            sqlx::query("UPDATE software_instances SET config = ?, updated_at = ? WHERE id = ?")
                .bind(&config_str)
                .bind(now())
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        }
        if let Some(m) = mcp_enabled {
            sqlx::query(
                "UPDATE software_instances SET mcp_enabled = ?, updated_at = ? WHERE id = ?",
            )
            .bind(m as i64)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        if let Some(a) = approval_mode {
            sqlx::query(
                "UPDATE software_instances SET approval_mode = ?, updated_at = ? WHERE id = ?",
            )
            .bind(a)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        self.get(id).await
    }

    pub async fn toggle(&self, id: i64, enabled: bool) -> Result<SoftwareInstance, String> {
        sqlx::query("UPDATE software_instances SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled as i64)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        self.get(id).await
    }

    pub async fn delete(&self, id: i64) -> Result<(), String> {
        sqlx::query("DELETE FROM software_instances WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 审计日志（best-effort，失败不阻断工具调用）。
    pub async fn insert_tool_log(&self, instance: &SoftwareInstance, entry: &ToolLogEntry) {
        let _ = sqlx::query(
            "INSERT INTO software_tool_logs (instance_id, instance_name, tool, risk, args_summary, status, error, duration_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(instance.id)
        .bind(&instance.name)
        .bind(&entry.tool)
        .bind(&entry.risk)
        .bind(&entry.args_summary)
        .bind(&entry.status)
        .bind(&entry.error)
        .bind(entry.duration_ms)
        .execute(&self.pool)
        .await;
    }

    pub async fn list_tool_logs(
        &self,
        instance_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let limit = limit.clamp(1, 200);
        let rows: Vec<ToolLogRow> = if let Some(id) = instance_id {
            sqlx::query_as(
                "SELECT id, tool, risk, args_summary, status, error, duration_ms, created_at \
                 FROM software_tool_logs WHERE instance_id = ? ORDER BY id DESC LIMIT ?",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?
        } else {
            sqlx::query_as(
                "SELECT id, tool, risk, args_summary, status, error, duration_ms, created_at \
                 FROM software_tool_logs ORDER BY id DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.0, "tool": row.1, "risk": row.2, "args_summary": row.3,
                    "status": row.4, "error": row.5, "duration_ms": row.6,
                    "created_at": row.7,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory db");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        pool
    }

    fn log_entry(tool: &str, status: &str) -> ToolLogEntry {
        ToolLogEntry {
            tool: tool.to_string(),
            risk: "read_only".to_string(),
            args_summary: "{}".to_string(),
            status: status.to_string(),
            error: if status == "error" {
                "boom".to_string()
            } else {
                String::new()
            },
            duration_ms: 12,
        }
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let repo = SoftwareRepository::new(pool().await);
        let created = repo
            .create(
                "generic_cli",
                "微信",
                &serde_json::json!({"commands": []}),
                false,
                "confirm_high_risk",
            )
            .await
            .expect("create");
        assert_eq!(created.connector_id, "generic_cli");
        assert_eq!(created.config["commands"], serde_json::json!([]));

        let updated = repo
            .update(created.id, None, None, Some(true), Some("auto"))
            .await
            .expect("update");
        assert_eq!(updated.mcp_enabled, 1);
        assert_eq!(updated.approval_mode, "auto");

        assert_eq!(repo.list_exposed().await.unwrap().len(), 1);

        let toggled = repo.toggle(created.id, false).await.unwrap();
        assert_eq!(toggled.enabled, 0);
        assert!(repo.list_exposed().await.unwrap().is_empty());

        repo.delete(created.id).await.unwrap();
        assert!(repo.get(created.id).await.is_err());
    }

    #[tokio::test]
    async fn tool_logs_roundtrip() {
        let repo = SoftwareRepository::new(pool().await);
        let inst = repo
            .create("generic_cli", "t", &serde_json::json!({}), false, "auto")
            .await
            .unwrap();
        repo.insert_tool_log(&inst, &log_entry("run", "ok")).await;
        repo.insert_tool_log(&inst, &log_entry("run", "error"))
            .await;
        let logs = repo.list_tool_logs(Some(inst.id), 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0]["status"], "error");
    }
}
