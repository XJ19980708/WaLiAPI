use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerStatus {
    pub running: bool,
    pub port: u16,
    pub url: String,
}

#[tauri::command]
pub async fn get_server_status(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<ServerStatus, String> {
    get_server_status_impl(&*state).await
}

pub async fn get_server_status_impl(state: &Arc<AppState>) -> Result<ServerStatus, String> {
    let running = state
        .server_running
        .load(std::sync::atomic::Ordering::SeqCst);
    let port = *state.server_port.read().await;
    Ok(ServerStatus {
        running,
        port,
        url: format!("http://127.0.0.1:{}", port),
    })
}

#[tauri::command]
pub async fn restart_server(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    // Stop existing server
    let mut handle_guard = state.server_handle.write().await;
    if let Some(handle) = handle_guard.take() {
        handle.abort();
    }
    state
        .server_running
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Start new server
    let app_clone = app.clone();
    let state_clone = state.inner().clone();
    let handle = tauri::async_runtime::spawn(async move {
        let _ = crate::server::start_server(state_clone, Some(app_clone)).await;
    });
    *handle_guard = Some(handle);

    Ok(())
}

/// 在线调试 /mcp 的返回（状态码 + 原始响应体）。
#[derive(Debug, Serialize)]
pub struct McpOnlineCallResult {
    pub status: u16,
    pub body: String,
}

/// 桌面前端「在线调用」代理：由 Rust 侧请求本机 /mcp，
/// 绕开 WKWebView 对 http:// 回环地址的混合内容/CORS 限制（TypeError: Load failed）。
#[tauri::command]
pub async fn mcp_online_call(
    state: tauri::State<'_, Arc<AppState>>,
    auth: String,
    body: String,
) -> Result<McpOnlineCallResult, String> {
    mcp_online_call_impl(&*state, auth, body).await
}

pub async fn mcp_online_call_impl(
    state: &Arc<AppState>,
    auth: String,
    body: String,
) -> Result<McpOnlineCallResult, String> {
    let port = *state.server_port.read().await;
    let url = format!("http://127.0.0.1:{}/mcp", port);
    // no_proxy：回环地址绝不走系统代理（历史问题：代理会劫持 127.0.0.1）
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", auth))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("请求失败：{}", e))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok(McpOnlineCallResult { status, body: text })
}
