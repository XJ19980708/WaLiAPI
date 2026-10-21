//! 用**真实旧库副本**验证 0.4.2 升级兼容（迁移编号重排不再 VersionMismatch / duplicate 闪退）。
//!
//! 通过环境变量 `WALIAPI_LEGACY_DB_DIR` 指向含 `waliapi.db` 的目录；
//! 未设置则跳过（CI 无该 fixture，不阻塞常规测试）。

#[tokio::test]
async fn legacy_db_upgrade_keeps_migration_records_consistent() {
    let Ok(dir) = std::env::var("WALIAPI_LEGACY_DB_DIR") else {
        eprintln!("skip: WALIAPI_LEGACY_DB_DIR 未设置");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    assert!(dir.join("waliapi.db").exists(), "旧库副本不存在: {dir:?}");

    // 完整启动路径：迁移前备份 → 重编号(backfill) → migrate → 幂等补列。
    // 修复前这里会因 VersionMismatch/duplicate column 触发 panic 闪退。
    let db = waliapi_lib::db::Database::new_with_path(&dir).await;

    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT version, description FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&db.pool)
            .await
            .expect("读取迁移记录失败");

    let find = |v: i64| {
        rows.iter()
            .find(|(ver, _)| *ver == v)
            .map(|(_, d)| d.clone())
    };

    // 旧 047/048/049（我的早期 Wiki 迁移）已被重编号为 050/051/052
    assert_eq!(find(50).as_deref(), Some("api key wiki access"));
    assert_eq!(find(51).as_deref(), Some("wiki pdf ocr"));
    assert_eq!(find(52).as_deref(), Some("wiki kb reasoning temperature"));
    // 上游 047/048（request_log）在旧库原本空缺，应正常补跑成功
    assert_eq!(find(47).as_deref(), Some("request log client name"));
    assert_eq!(find(48).as_deref(), Some("request log observability"));
    // 版本 47 不应再残留我的旧迁移描述（那会导致 VersionMismatch）
    assert_ne!(find(47).as_deref(), Some("api key wiki access"));

    // 关键列/表存在（迁移与补齐均生效）
    let cols: Vec<String> = sqlx::query("PRAGMA table_info(wiki_projects)")
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .filter_map(|r| sqlx::Row::try_get::<String, _>(r, "name").ok())
        .collect();
    for c in [
        "pdf_ocr_channel_id",
        "pdf_ocr_model",
        "reasoning_effort",
        "temperature",
    ] {
        assert!(cols.iter().any(|x| x == c), "wiki_projects 缺列 {c}");
    }
}
