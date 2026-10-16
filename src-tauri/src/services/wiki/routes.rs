use super::handlers;
use crate::server::router::SharedState;
use crate::AppState;
use axum::routing::{delete, get, post};
use axum::Router;
use std::sync::Arc;

pub fn create_router(_state: Arc<AppState>) -> Router<SharedState> {
    Router::new()
        // ── Project management ──
        .route(
            "/api/wiki/projects",
            get(handlers::list_projects).post(handlers::create_project),
        )
        .route(
            "/api/wiki/projects/{id}",
            get(handlers::get_project)
                .put(handlers::update_project)
                .delete(handlers::delete_project),
        )
        .route(
            "/api/wiki/projects/{id}/stats",
            get(handlers::get_project_stats),
        )
        // ── Sources ──
        .route(
            "/api/wiki/projects/{id}/sources",
            get(handlers::list_sources).post(handlers::add_source),
        )
        .route(
            "/api/wiki/projects/{id}/sources/{sid}",
            delete(handlers::delete_source),
        )
        .route(
            "/api/wiki/projects/{id}/sources/{sid}/ingest",
            post(handlers::ingest_source),
        )
        .route(
            "/api/wiki/projects/{id}/rescan",
            post(handlers::rescan_sources),
        )
        // ── Wiki pages ──
        .route("/api/wiki/projects/{id}/pages", get(handlers::list_pages))
        .route(
            "/api/wiki/projects/{id}/pages/{*path}",
            get(handlers::get_page)
                .put(handlers::update_page)
                .delete(handlers::delete_page),
        )
        // ── Search & Ask ──
        .route("/api/wiki/projects/{id}/search", get(handlers::search))
        .route("/api/wiki/projects/{id}/ask", post(handlers::ask))
        // ── Graph ──
        .route("/api/wiki/projects/{id}/graph", get(handlers::get_graph))
        // ── Sessions ──
        .route(
            "/api/wiki/projects/{id}/sessions",
            get(handlers::list_sessions).delete(handlers::clear_sessions),
        )
        // ── Queue ──
        .route(
            "/api/wiki/projects/{id}/queue",
            get(handlers::get_queue_status),
        )
}

/// Wiki 图像资产路由（只读、免鉴权、token 即凭证）。
/// 独立于 create_router 的读写鉴权层：`<img>` 静态资源请求无法携带 Bearer，
/// 安全性由「摄入时逐页生成的随机 token 路径 + 白名单字符校验 + 不记访问日志」保证
/// （用户已确认「内部资料 / 直连」档位）。
pub fn assets_router() -> Router<SharedState> {
    Router::new().route(
        "/api/wiki/projects/{id}/assets/{token}/{*file}",
        get(handlers::get_asset),
    )
}
