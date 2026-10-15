//! T09 request-log observability tests.
//!
//! Covers:
//!   * migration 016 adds the 11 nullable observability columns and keeps the
//!     old queries (search/aggregate) working;
//!   * `Repository::create_log` persists the new fields (and NULLs for legacy
//!     callers);
//!   * `LogDto` maps the new fields (client_cancelled / stream_committed as
//!     booleans);
//!   * old-log compatibility: a row written with NULL observability columns
//!     still parses and maps to `LogDto` with `None` (old frontend types stay
//!     nullable);
//!   * a row with the new fields round-trips per-field.

use serde_json::json;
use waliapi_lib::{
    db::{models, repository::Repository},
    security::{gate::gate_original, SecuritySettings},
};

fn now() -> String {
    models::now_iso()
}

/// In-memory SQLite with all migrations (incl. 016) applied.
async fn fresh_db() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory db");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate fresh db");
    pool
}

/// Build a full `RequestLog` with the T09 observability fields populated.
fn full_log(channel_id: Option<&str>, channel_name: Option<&str>) -> models::RequestLog {
    models::RequestLog {
        id: waliapi_lib::utils::id::new_id(),
        seq: None,
        api_key_id: Some("key-1".into()),
        api_key_name: Some("tester".into()),
        channel_id: channel_id.map(|s| s.to_string()),
        channel_name: channel_name.map(|s| s.to_string()),
        model: "alias".into(),
        upstream_model: Some("upstream-x".into()),
        mode: "chat".into(),
        status_code: 200,
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        cached_tokens: 0,
        duration_ms: 120,
        error_message: None,
        is_stream: 1,
        is_retry: 0,
        created_at: now(),
        request_body: Some(json!({"model":"alias","messages":[]}).to_string()),
        response_choices: None,
        risk_level: "clean".into(),
        risk_score: 0,
        risk_summary: Some("ok".into()),
        security_action: "allow".into(),
        sanitized: 1,
        blocked_reason: None,
        trace_id: Some("trace-1".into()),
        // --- T09 observability ---
        reasoning_effort: None,
        downstream_protocol: Some("chat_completions".into()),
        downstream_endpoint: Some("chat_completions".into()),
        route_group: Some("chat_completions_g1_native".into()),
        upstream_protocol: Some("openai".into()),
        upstream_endpoint: Some("chat_completions".into()),
        provider: Some("deepseek".into()),
        codec_version: None,
        failure_class: None,
        identity_revision: Some(1),
        client_cancelled: Some(0),
        stream_committed: Some(1),
        upstream_type: "channel".into(),
        client_name: Some("test-client".into()),
    }
}

#[tokio::test]
async fn request_log_migration_016_adds_nullable_columns_and_old_queries_still_work() {
    let pool = fresh_db().await;
    // All 11 new columns exist and are nullable.
    for col in [
        "downstream_protocol",
        "downstream_endpoint",
        "route_group",
        "upstream_protocol",
        "upstream_endpoint",
        "provider",
        "codec_version",
        "failure_class",
        "identity_revision",
        "client_cancelled",
        "stream_committed",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('request_logs') WHERE name = ?",
        )
        .bind(col)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "column {col} must exist from migration 016");
    }
    // A legacy-style row (old columns only) parses through the repository
    // (FromRow with NULL observability columns) and aggregate queries still run.
    let repo = Repository::new(pool);
    let legacy = models::RequestLog {
        id: waliapi_lib::utils::id::new_id(),
        seq: None,
        api_key_id: Some("key-1".into()),
        api_key_name: Some("legacy".into()),
        channel_id: None,
        channel_name: Some("old-channel".into()),
        model: "m".into(),
        upstream_model: None,
        mode: "chat".into(),
        status_code: 200,
        prompt_tokens: 7,
        completion_tokens: 3,
        total_tokens: 10,
        duration_ms: 50,
        error_message: None,
        is_stream: 0,
        is_retry: 0,
        created_at: now(),
        request_body: None,
        response_choices: None,
        risk_level: "clean".into(),
        risk_score: 0,
        risk_summary: None,
        security_action: "allow".into(),
        sanitized: 1,
        blocked_reason: None,
        trace_id: None,
        ..Default::default() // observability columns => NULL
    };
    repo.create_log(&legacy).await.expect("legacy log insert");
    repo.create_log(&full_log(None, Some("native")))
        .await
        .expect("full log insert");

    // Old list/search queries remain compatible.
    let logs = repo.get_logs(50, 0).await.expect("get_logs");
    assert_eq!(logs.len(), 2);
    let found = repo
        .search_logs(Some("legacy"), None, None, None, None, None, None, 50, 0)
        .await
        .expect("search_logs");
    assert_eq!(found.len(), 1);

    // Aggregate queries still work (channel stats / dashboard / log stats).
    let _ = repo.get_dashboard_stats().await.expect("dashboard stats");
    let _ = repo.get_channel_stats().await.expect("channel stats");
    let _ = repo.get_log_stats(7).await.expect("log stats");
}

#[tokio::test]
async fn request_log_create_log_persists_t09_fields_and_log_dto_maps_them() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);
    let log = full_log(Some("ch-1"), Some("native-channel"));
    repo.create_log(&log).await.expect("create_log");

    let stored = repo.get_log(&log.id).await.expect("get_log");
    assert_eq!(
        stored.downstream_protocol.as_deref(),
        Some("chat_completions")
    );
    assert_eq!(
        stored.downstream_endpoint.as_deref(),
        Some("chat_completions")
    );
    assert_eq!(
        stored.route_group.as_deref(),
        Some("chat_completions_g1_native")
    );
    assert_eq!(stored.upstream_protocol.as_deref(), Some("openai"));
    assert_eq!(
        stored.upstream_endpoint.as_deref(),
        Some("chat_completions")
    );
    assert_eq!(stored.provider.as_deref(), Some("deepseek"));
    assert_eq!(stored.codec_version, None);
    assert_eq!(stored.failure_class, None);
    assert_eq!(stored.identity_revision, Some(1));
    assert_eq!(stored.client_cancelled, Some(0));
    assert_eq!(stored.stream_committed, Some(1));

    let dto: waliapi_lib::commands::log::LogDto = stored.into();
    assert_eq!(
        dto.route_group.as_deref(),
        Some("chat_completions_g1_native")
    );
    assert_eq!(dto.upstream_protocol.as_deref(), Some("openai"));
    assert_eq!(dto.provider.as_deref(), Some("deepseek"));
    assert_eq!(dto.identity_revision, Some(1));
    assert_eq!(dto.client_cancelled, Some(false));
    assert_eq!(dto.stream_committed, Some(true));
}

#[tokio::test]
async fn request_log_summary_reports_size_without_loading_body() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);
    let mut log = full_log(None, Some("summary"));
    log.id = "summary-large".into();
    log.request_body = Some("x".repeat(1024 * 1024));
    repo.create_log(&log).await.expect("create_log");

    let summaries = repo.get_log_summaries(20, 0).await.expect("summaries");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, log.id);
    assert!(summaries[0].has_request_body);
    assert_eq!(summaries[0].request_body_bytes, 1024 * 1024);
}

#[tokio::test]
async fn request_log_basic_policy_drops_bodies_at_persistence_boundary() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);
    let mut log = full_log(None, Some("basic"));
    log.id = "basic-log".into();
    log.response_choices = Some("{\"choices\":[]}".into());
    repo.create_log_with_policy(
        &log,
        waliapi_lib::audit_log::LogPolicy {
            detail_level: waliapi_lib::audit_log::LogDetailLevel::Basic,
            retention_days: 7,
        },
    )
    .await
    .expect("create basic log");

    let stored = repo.get_log(&log.id).await.expect("get basic log");
    assert!(stored.request_body.is_none());
    assert!(stored.response_choices.is_none());
    let level: String = sqlx::query_scalar("SELECT detail_level FROM request_logs WHERE id = ?")
        .bind(&log.id)
        .fetch_one(repo.pool())
        .await
        .unwrap();
    assert_eq!(level, "basic");
}

#[tokio::test]
async fn request_log_upstream_type_defaults_filters_and_round_trips() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);

    let channel_log = full_log(Some("channel-1"), Some("API channel"));
    assert_eq!(channel_log.upstream_type, "channel");
    repo.create_log(&channel_log).await.expect("channel log");

    let mut account_log = full_log(Some("account-1"), Some("Codex account"));
    account_log.upstream_type = "auth_account".into();
    repo.create_log(&account_log).await.expect("account log");

    let stored = repo
        .get_log(&account_log.id)
        .await
        .expect("account log read");
    assert_eq!(stored.upstream_type, "auth_account");
    let dto: waliapi_lib::commands::log::LogDto = stored.into();
    assert_eq!(dto.upstream_type, "auth_account");

    let channel_only = repo
        .search_logs_by_upstream_type(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("channel"),
            50,
            0,
        )
        .await
        .expect("channel filter");
    assert_eq!(channel_only.len(), 1);
    assert_eq!(channel_only[0].id, channel_log.id);

    let account_only = repo
        .search_logs_by_upstream_type(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("auth_account"),
            50,
            0,
        )
        .await
        .expect("account filter");
    assert_eq!(account_only.len(), 1);
    assert_eq!(account_only[0].id, account_log.id);
}

#[tokio::test]
async fn request_log_legacy_log_with_null_observability_maps_to_none_in_dto() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);
    let legacy = models::RequestLog {
        id: waliapi_lib::utils::id::new_id(),
        seq: None,
        api_key_id: None,
        api_key_name: None,
        channel_id: None,
        channel_name: None,
        model: "m".into(),
        upstream_model: None,
        mode: "chat".into(),
        status_code: 502,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        duration_ms: 0,
        error_message: Some("legacy error".into()),
        is_stream: 0,
        is_retry: 0,
        created_at: now(),
        request_body: None,
        response_choices: None,
        risk_level: "clean".into(),
        risk_score: 0,
        risk_summary: None,
        security_action: "allow".into(),
        sanitized: 1,
        blocked_reason: None,
        trace_id: None,
        ..Default::default() // observability columns => NULL
    };
    repo.create_log(&legacy).await.expect("create_log");
    let stored = repo.get_log(&legacy.id).await.expect("get_log");

    // Old frontend (nullable fields) stays compatible: every new field is None.
    assert_eq!(stored.route_group, None);
    assert_eq!(stored.upstream_protocol, None);
    assert_eq!(stored.identity_revision, None);
    assert_eq!(stored.client_cancelled, None);
    assert_eq!(stored.stream_committed, None);

    let dto: waliapi_lib::commands::log::LogDto = stored.into();
    assert_eq!(dto.route_group, None);
    assert_eq!(dto.client_cancelled, None);
    assert_eq!(dto.stream_committed, None);
    assert_eq!(dto.failure_class, None);
}

#[tokio::test]
async fn request_log_sanitized_log_body_is_what_gets_persisted() {
    // The gate's `sanitized_log_json` is the ONLY body source for logging; raw
    // body never reaches the log.  Verify the plumbing: the field we persist is
    // the sanitized log body string, and the raw secret is absent.
    let raw = json!({"model": "m", "messages": [{"role": "user", "content": "Bearer sk-abcdefghijklmnopqrstuvwxyz123456"}]});
    let audited = gate_original(
        waliapi_lib::security::gate::DownstreamProtocol::ChatCompletions,
        "/v1/chat/completions",
        raw.clone(),
        None,
        "m".to_string(),
        false,
        None,
        &SecuritySettings::default(),
        None,
        vec![],
    )
    .expect("gate");
    let log_body = serde_json::to_string(&audited.sanitized_log_json).unwrap();
    assert!(!log_body.contains("abcdefghijklmnopqrstuvwx"));
    assert!(serde_json::to_string(&raw)
        .unwrap()
        .contains("abcdefghijklmnopqrstuvwx"));
}

#[tokio::test]
async fn request_log_cleanup_removes_expired_rows_and_findings() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool.clone());
    let mut old = full_log(None, Some("old"));
    old.id = "old-log".into();
    old.created_at = "2000-01-01T00:00:00Z".into();
    repo.create_log(&old).await.expect("create old log");
    sqlx::query(
        "INSERT INTO request_security_findings
         (id, log_id, phase, category, rule_id, severity, title, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("finding-old")
    .bind(&old.id)
    .bind("request")
    .bind("test")
    .bind("test-rule")
    .bind("low")
    .bind("test")
    .bind("2000-01-01T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert finding");

    let deleted = waliapi_lib::audit_log::cleanup_expired_logs(&pool, 1)
        .await
        .expect("cleanup");
    assert_eq!(deleted, 1);
    assert!(repo.get_log(&old.id).await.is_err());
    let findings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM request_security_findings WHERE log_id = ?")
            .bind(&old.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(findings, 0);
    assert_eq!(
        waliapi_lib::audit_log::cleanup_expired_logs(&pool, 0)
            .await
            .unwrap(),
        0
    );
}

// ─── Stream segments（迁移 032：流式内容段溢出表）────────────────────────────

fn detailed_policy() -> waliapi_lib::audit_log::LogPolicy {
    waliapi_lib::audit_log::LogPolicy {
        detail_level: waliapi_lib::audit_log::LogDetailLevel::Detailed,
        retention_days: 7,
    }
}

fn brief_policy() -> waliapi_lib::audit_log::LogPolicy {
    waliapi_lib::audit_log::LogPolicy {
        detail_level: waliapi_lib::audit_log::LogDetailLevel::Brief,
        retention_days: 7,
    }
}

#[tokio::test]
async fn stream_segments_persisted_only_under_detailed_policy_for_streams() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);

    // detailed + 流式 + 有内容 → 落段
    let mut log = full_log(None, Some("seg-1"));
    log.id = "stream-detailed".into();
    log.response_choices =
        Some("{\"choices\":[{\"message\":{\"content\":\"部分生成内容\"}}]}".into());
    repo.create_log_with_policy(&log, detailed_policy())
        .await
        .unwrap();
    let segments = repo.get_stream_segments("stream-detailed").await.unwrap();
    assert_eq!(segments.len(), 1, "detailed 流式应落一段");
    assert_eq!(segments[0].0, 1);
    assert!(segments[0].1.contains("部分生成内容"));

    // basic + 流式 + 有内容 → 不落段（尊重用户存储选择）
    let mut log = full_log(None, Some("seg-2"));
    log.id = "stream-basic".into();
    log.response_choices = Some("{\"choices\":[]}".into());
    repo.create_log_with_policy(
        &log,
        waliapi_lib::audit_log::LogPolicy {
            detail_level: waliapi_lib::audit_log::LogDetailLevel::Basic,
            retention_days: 7,
        },
    )
    .await
    .unwrap();
    assert!(
        repo.get_stream_segments("stream-basic")
            .await
            .unwrap()
            .is_empty(),
        "basic 不落段"
    );

    // detailed + 非流式 → 不落段
    let mut log = full_log(None, Some("seg-3"));
    log.id = "non-stream".into();
    log.is_stream = 0;
    log.response_choices = Some("{\"choices\":[]}".into());
    repo.create_log_with_policy(&log, detailed_policy())
        .await
        .unwrap();
    assert!(
        repo.get_stream_segments("non-stream")
            .await
            .unwrap()
            .is_empty(),
        "非流式不落段"
    );

    // detailed + 流式 + 内容为空 → 不落段
    let mut log = full_log(None, Some("seg-4"));
    log.id = "stream-empty".into();
    log.response_choices = None;
    repo.create_log_with_policy(&log, detailed_policy())
        .await
        .unwrap();
    assert!(
        repo.get_stream_segments("stream-empty")
            .await
            .unwrap()
            .is_empty(),
        "无累计内容不落段"
    );
}

#[tokio::test]
async fn stream_segments_purged_by_all_delete_paths() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);

    // 一条“过期”日志与一条“新”日志，均带段
    let mut old = full_log(None, Some("old"));
    old.id = "old-log".into();
    old.created_at = "2020-01-01T00:00:00+00:00".into();
    old.response_choices = Some("old-content".into());
    let mut new = full_log(None, Some("new"));
    new.id = "new-log".into();
    new.created_at = now();
    new.response_choices = Some("new-content".into());
    repo.create_log_with_policy(&old, detailed_policy())
        .await
        .unwrap();
    repo.create_log_with_policy(&new, detailed_policy())
        .await
        .unwrap();

    // 按日期清理：只清旧的（段随主行走）
    repo.delete_logs_before("2021-01-01T00:00:00+00:00")
        .await
        .unwrap();
    assert!(repo
        .get_stream_segments("old-log")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(repo.get_stream_segments("new-log").await.unwrap().len(), 1);

    // 单条删除：段随行走
    repo.delete_log("new-log").await.unwrap();
    assert!(repo
        .get_stream_segments("new-log")
        .await
        .unwrap()
        .is_empty());

    // 全量清空
    let mut again = full_log(None, Some("again"));
    again.id = "again-log".into();
    again.response_choices = Some("again-content".into());
    repo.create_log_with_policy(&again, detailed_policy())
        .await
        .unwrap();
    repo.delete_all_logs().await.unwrap();
    assert!(repo
        .get_stream_segments("again-log")
        .await
        .unwrap()
        .is_empty());
}

/// 关键不变式：「简要」只在**落库漏斗**上裁消息列表，绝不能碰到转发侧看到的请求记录。
///
/// 下游智能体（Codex / Claude Code）单次提交常带上百条历史消息，网关必须把它们**原样
/// 转发**给上游供应商；"只留 3 条"仅是审计日志的存储与展示裁剪。本测试走真实的
/// `create_log_with_policy`（内存 SQLite + 全量迁移），同时钉住四件事：
///   1. 调用方持有的 `RequestLog` 逐字节不变（截断作用于克隆）；
///   2. 落库的那一行才是裁到 3 条的、并带 `_wali_brief` 标记；
///   3. 统计字段（token / 状态码 / 耗时 / 密钥 / 渠道）一个不少；
///   4. 响应正文与流式内容段照旧落库（请求侧的裁剪不得牵连响应侧）。
#[tokio::test]
async fn brief_policy_truncates_only_the_stored_copy_not_the_forwarded_record() {
    let pool = fresh_db().await;
    let repo = Repository::new(pool);

    // 模拟一次几百条消息的真实提交（这里 5 条即可复现语义，另加填充保证体积可辨）
    let filler = "x".repeat(200);
    let messages: Vec<serde_json::Value> = (0..5)
        .map(|i| json!({"role": "user", "content": format!("m{}{}", i, filler)}))
        .collect();
    let original = json!({"model": "alias", "messages": messages}).to_string();

    let mut log = full_log(Some("ch-1"), Some("ch-a"));
    log.id = "brief-log".into();
    log.request_body = Some(original.clone());
    let response = json!({"choices": [{"message": {"content": "FULL ANSWER"}}]}).to_string();
    log.response_choices = Some(response.clone());

    repo.create_log_with_policy(&log, brief_policy())
        .await
        .expect("create brief log");

    // 1) 转发侧的记录必须完好无损 —— 这是本测试存在的理由
    assert_eq!(
        log.request_body.as_deref(),
        Some(original.as_str()),
        "落库截断只能作用于克隆：调用方（请求转发路径）手里的原始正文不能被裁短"
    );

    // 2) 落库行才是被裁过的
    let stored = repo.get_log(&log.id).await.expect("get brief log");
    let body: serde_json::Value =
        serde_json::from_str(stored.request_body.as_ref().expect("简要仍应保存正文")).unwrap();
    let kept = body["messages"].as_array().expect("messages 应仍是数组");
    assert_eq!(kept.len(), 3, "落库只应保留最新 3 条");
    assert!(
        kept[0]["content"].as_str().unwrap().starts_with("m2"),
        "被丢掉的应是最旧的消息"
    );
    assert_eq!(body["_wali_brief"]["omitted_messages"], json!(2));

    // 3) 统计口径不受影响
    assert_eq!(stored.total_tokens, 15);
    assert_eq!(stored.prompt_tokens, 10);
    assert_eq!(stored.completion_tokens, 5);
    assert_eq!(stored.status_code, 200);
    assert_eq!(stored.duration_ms, 120);
    assert_eq!(stored.api_key_id.as_deref(), Some("key-1"));
    assert_eq!(stored.channel_id.as_deref(), Some("ch-1"));

    // 4) 响应侧完整保留：正文照存、流式内容段照落
    assert_eq!(stored.response_choices.as_deref(), Some(response.as_str()));
    let segments = repo.get_stream_segments(&log.id).await.unwrap();
    assert_eq!(
        segments.len(),
        1,
        "brief 下流式内容段必须照落，不受请求侧裁剪牵连"
    );
    assert!(segments[0].1.contains("FULL ANSWER"));

    let level: String = sqlx::query_scalar("SELECT detail_level FROM request_logs WHERE id = ?")
        .bind(&log.id)
        .fetch_one(repo.pool())
        .await
        .unwrap();
    assert_eq!(level, "brief");
}
