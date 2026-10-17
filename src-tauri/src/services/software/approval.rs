//! 高危操作审批 —— 桌面端原生弹窗确认。
//!
//! - 桌面构建（desktop-ui）+ AppHandle 可用：弹出系统级 Ok/Cancel 弹窗，
//!   用户点击「允许执行」才放行；
//! - headless 构建（无 UI）：一律拒绝，并在错误信息中说明改法
//!   （把实例审批模式改为 `auto`，或改用桌面端操作）。

use std::sync::Arc;
use tokio::sync::RwLock;

/// AppState 中保存的 AppHandle 类型别名。
pub type SharedAppHandle = Arc<RwLock<Option<tauri::AppHandle>>>;

/// 请求用户确认。返回 true = 允许执行。
pub async fn request_confirmation(
    app_handle: &SharedAppHandle,
    title: &str,
    message: &str,
) -> bool {
    #[cfg(feature = "desktop-ui")]
    {
        let handle = app_handle.read().await.clone();
        let Some(handle) = handle else {
            return false;
        };
        let title = title.to_string();
        let message = message.to_string();
        // blocking_show 会阻塞当前线程，放到 blocking 池避免占用 worker
        tokio::task::spawn_blocking(move || {
            use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
            handle
                .dialog()
                .message(message)
                .title(title)
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "允许执行".to_string(),
                    "拒绝".to_string(),
                ))
                .blocking_show()
        })
        .await
        .unwrap_or(false)
    }

    #[cfg(not(feature = "desktop-ui"))]
    {
        let _ = (app_handle, title, message);
        false
    }
}
