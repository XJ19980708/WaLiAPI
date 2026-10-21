//! 微信连接器（macOS，实验性）。
//!
//! 微信没有官方本地 API，本连接器基于**系统脚本桥**（路径 B）+ UI 自动化（路径 C）：
//! - 读侧：进程/运行状态检测（稳定）
//! - 写侧：激活窗口 → Cmd+F 搜索联系人 → 剪贴板粘贴（规避 CJK 键入问题）→ 回车发送
//!
//! 前置条件（桌面端 UI 有引导）：
//! - WaLiAPI 需要被授予「辅助功能」权限（系统设置 → 隐私与安全性 → 辅助功能）
//! - 微信需保持登录
//!
//! 风险：微信界面结构随版本变化可能导致搜索/发送流程失效；`send_text` 属高危写，
//! 默认走桌面弹窗审批。Windows 端不内置（建议配合 wxauto 等外置方案走通用命令行）。

use super::super::connector::{RiskLevel, ToolManifest};
use super::super::executor;
use super::super::model::SoftwareInstance;
use crate::services::software::connector::SoftwareConnector;
use async_trait::async_trait;
use std::time::Duration;

pub struct WeChatConnector;

/// AppleScript 字符串转义（反斜杠 + 双引号）。
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn app_name_of(instance: &SoftwareInstance) -> String {
    instance
        .config
        .get("app_name")
        .and_then(|v| v.as_str())
        .unwrap_or("WeChat")
        .to_string()
}

fn delay_of(instance: &SoftwareInstance, key: &str, default_ms: u64) -> f64 {
    instance
        .config
        .get(key)
        .and_then(|v| v.as_u64())
        .unwrap_or(default_ms)
        .clamp(100, 10_000) as f64
        / 1000.0
}

/// 把文本放上剪贴板（经 osascript，规避 CJK keystroke 问题）。
async fn set_clipboard(text: &str) -> Result<(), String> {
    let script = format!("set the clipboard to \"{}\"", escape_applescript(text));
    let out = executor::exec(
        "osascript",
        &["-e".into(), script],
        None,
        Duration::from_secs(10),
    )
    .await;
    if out.success {
        Ok(())
    } else {
        Err(format!("写入剪贴板失败: {}", out.stderr))
    }
}

/// 执行一段 System Events UI 脚本。
async fn run_ui_script(script: String) -> Result<(), String> {
    let out = executor::exec(
        "osascript",
        &["-e".into(), script],
        None,
        Duration::from_secs(30),
    )
    .await;
    if out.success {
        Ok(())
    } else if out.stderr.contains("not allowed assistive access") {
        Err("系统拒绝辅助功能访问：请在 系统设置 → 隐私与安全性 → 辅助功能 中授权 WaLiAPI，然后重试".into())
    } else {
        Err(format!("UI 脚本执行失败: {}", out.stderr))
    }
}

/// BFS 批量读取微信窗口辅助功能树的可见文本。
/// 关键性能点：每层容器用 `value/title of every UI element of X` 一次 Apple Event
/// 批量取值，避免逐元素递归（整树逐元素会轻松超过 60s 超时）。
/// 微信各版本 UI 结构差异大，这里不做结构假设，宁可多读再由调用方筛选。
/// 读取微信会话列表文本。
/// 策略：先浅层 BFS 找 AXOutline/AXTable（微信会话列表/消息区都是表格类容器），
/// 再用嵌套 `every` 语法对每个表格**单次 Apple Event** 拉取全部行的全部静态文本
/// （服务端遍历，避免逐元素往返）。找不到表格时退化为受限的逐层文本收集。
const DUMP_TREE_SCRIPT: &str = r#"
on flatten(l)
  set out to ""
  repeat with x in l
    try
      set c to contents of x
      if class of c is list then
        set out to out & my flatten(c)
      else
        set t to c as text
        if t is not "" and (length of t) < 300 then set out to out & t & linefeed
      end if
    end try
  end repeat
  return out
end flatten

on joinList(lst)
  set AppleScript's text item delimiters to linefeed
  set t to lst as text
  set AppleScript's text item delimiters to ""
  return t
end joinList

on tableText(t)
  tell application "System Events"
    set parts to ""
    try
      set parts to parts & my flatten(value of every static text of every UI element of every row of t)
    end try
    try
      set parts to parts & my flatten(value of every static text of t)
    end try
    try
      set parts to parts & my flatten(title of every UI element of every row of t)
    end try
  end tell
  return parts
end tableText

tell application "System Events" to tell process "WeChat"
  -- 前置检查：微信主窗口必须存在（微信常驻菜单栏时主窗口往往是关的）
  -- 注意：辅助功能权限被拒（-10004/-25211/-1719）也会让 count 失败，
  -- 必须原样抛出让上层映射为授权指引，不能吞成 WECHAT_NO_WINDOW
  set wc to 0
  try
    set wc to count of windows
  on error errMsg number errNum
    if errNum is -10004 or errNum is -25211 or errNum is -1719 then error errMsg number errNum
  end try
  if wc is 0 then error "WECHAT_NO_WINDOW"
  -- 阶段一：浅层 BFS 找表格类容器（只下钻 Group/SplitGroup/ScrollArea/TabGroup）
  set tables to {}
  set frontier to {window 1}
  try
    repeat 6 times
      set nextFrontier to {}
      set nextRoles to {}
      repeat with p in frontier
        try
          set rs to role of every UI element of p
          set ks to every UI element of p
          repeat with i from 1 to count of ks
            set r to ""
            try
              set r to (item i of rs) as text
            end try
            set k to contents of item i of ks
            if r is "AXOutline" or r is "AXTable" then
              if (count of tables) < 3 then set end of tables to k
            else if r is "AXGroup" or r is "AXSplitGroup" or r is "AXScrollArea" or r is "AXTabGroup" then
              set end of nextFrontier to k
            end if
          end repeat
        end try
      end repeat
      if (count of nextFrontier) is 0 then exit repeat
      if (count of nextFrontier) > 120 then set nextFrontier to items 1 thru 120 of nextFrontier
      set frontier to nextFrontier
    end repeat
  on error errMsg number errNum
    if errNum is not -609 and errNum is not -1728 then error errMsg number errNum
  end try

  -- 阶段二：对每个表格单事件批量拉取全部行文本
  set allOut to ""
  repeat with t in tables
    try
      set allOut to allOut & my tableText(t)
    end try
  end repeat

  -- 阶段三兜底：没找到表格则浅层收集窗口可见文本
  if allOut is "" then
    try
      set allOut to allOut & my flatten(value of every static text of window 1)
    end try
  end if
  if allOut is "" then error "WeChat 窗口未读到可见文本（窗口可能未打开或界面不暴露辅助功能）"
  return allOut
end tell
"#;

/// 清洗 dump 结果：去空行、去重（保序）、截断超长行。
fn sanitize_dumped_lines(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.chars().count() > 200 {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
        if out.len() >= 200 {
            break;
        }
    }
    out
}

#[async_trait]
impl SoftwareConnector for WeChatConnector {
    fn id(&self) -> &'static str {
        "wechat"
    }
    fn label(&self) -> &'static str {
        "微信"
    }
    fn description(&self) -> &'static str {
        "微信桌面端实验性连接器：运行状态检测 + 读取可见会话信息（辅助功能树）+ 搜索联系人发送文本（剪贴板中转，发送走高危审批）。需辅助功能权限"
    }
    fn platforms(&self) -> &'static str {
        "macOS（实验性）；Windows 建议配合 wxauto 走通用命令行"
    }
    fn sample_config(&self) -> serde_json::Value {
        serde_json::json!({
            "app_name": "WeChat",
            "search_delay_ms": 900,
            "send_delay_ms": 600
        })
    }

    fn validate_config(&self, config: &serde_json::Value) -> Result<(), String> {
        if let Some(name) = config.get("app_name") {
            if !name.is_string() || name.as_str().unwrap_or("").trim().is_empty() {
                return Err(
                    "app_name 必须是非空字符串（macOS 应用名，如 WeChat / 微信）".to_string(),
                );
            }
        }
        for key in ["search_delay_ms", "send_delay_ms"] {
            if let Some(v) = config.get(key) {
                if !v.is_number() {
                    return Err(format!("{key} 必须是数字（毫秒）"));
                }
            }
        }
        Ok(())
    }

    async fn list_tools(&self, _instance: &SoftwareInstance) -> Result<Vec<ToolManifest>, String> {
        Ok(vec![
            ToolManifest::read_only(
                "check_running",
                "检测微信是否正在运行",
                serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            ToolManifest::read_only(
                "launch",
                "启动/唤起微信窗口",
                serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            ToolManifest::read_only(
                "list_chats",
                "读取微信主窗口可见的会话信息（会话名称与最近消息预览等可见文本）。微信需保持打开且已登录",
                serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            ToolManifest {
                name: "send_text".to_string(),
                description:
                    "给指定联系人/群聊发送一条文本消息（走搜索定位会话；高危操作，需用户确认）"
                        .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "contact": {"type": "string", "description": "联系人或群聊名称（微信搜索框可命中的关键字）"},
                        "message": {"type": "string", "description": "要发送的文本内容"}
                    },
                    "required": ["contact", "message"],
                    "additionalProperties": false
                }),
                risk: RiskLevel::HighWrite,
            },
        ])
    }

    async fn call_tool(
        &self,
        instance: &SoftwareInstance,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let app_name = app_name_of(instance);
        match tool {
            "check_running" => {
                let script = format!(
                    "application \"{}\" is running",
                    escape_applescript(&app_name)
                );
                let out = executor::exec(
                    "osascript",
                    &["-e".into(), script],
                    None,
                    Duration::from_secs(10),
                )
                .await;
                let running = out.stdout.trim() == "true";
                Ok(serde_json::json!({ "running": running, "success": true }))
            }
            "launch" => {
                let out = executor::exec(
                    "open",
                    &["-a".into(), app_name.clone()],
                    None,
                    Duration::from_secs(15),
                )
                .await;
                if out.success {
                    Ok(serde_json::json!({ "success": true, "stdout": out.stdout }))
                } else {
                    Err(format!("启动微信失败: {}", out.stderr))
                }
            }
            "list_chats" => {
                // 先唤起微信，避免主窗口未打开导致 AX 树无内容
                let _ = executor::exec(
                    "open",
                    &["-a".into(), app_name.clone()],
                    None,
                    Duration::from_secs(15),
                )
                .await;
                tokio::time::sleep(Duration::from_secs_f64(1.2)).await;
                let out = executor::exec(
                    "osascript",
                    &["-e".into(), DUMP_TREE_SCRIPT.to_string()],
                    None,
                    Duration::from_secs(60),
                )
                .await;
                if !out.success {
                    let err = out.stderr;
                    if err.contains("not allowed assistive access")
                        || err.contains("权限违例")
                        || err.contains("-10004")
                        || err.contains("-25211")
                    {
                        return Err("系统拒绝辅助功能访问：请在 系统设置 → 隐私与安全性 → 辅助功能 中授权运行 WaLiAPI 的终端/应用，然后重试".into());
                    }
                    if err.contains("WECHAT_NO_WINDOW") {
                        return Err("微信没有打开的主窗口（可能仅驻留菜单栏）：请调用 launch 工具后再试，或打开微信主窗口".into());
                    }
                    return Err(format!("读取微信窗口失败: {err}"));
                }
                let lines = sanitize_dumped_lines(&out.stdout);
                Ok(serde_json::json!({
                    "success": true,
                    "visible_texts": lines,
                    "note": "以上为微信主窗口可见文本（会话名、消息预览、联系人等），按界面布局顺序去重排列"
                }))
            }
            "send_text" => {
                let contact = args
                    .get("contact")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 contact")?;
                let message = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 message")?;
                if contact.is_empty() || message.is_empty() {
                    return Err("contact 与 message 不能为空".to_string());
                }
                let search_delay = delay_of(instance, "search_delay_ms", 900);
                let send_delay = delay_of(instance, "send_delay_ms", 600);

                // 1. 唤起微信
                let _ = executor::exec(
                    "open",
                    &["-a".into(), app_name.clone()],
                    None,
                    Duration::from_secs(15),
                )
                .await;
                tokio::time::sleep(Duration::from_secs_f64(0.8)).await;

                // 2. Cmd+F 搜索 → 粘贴联系人 → 回车进入会话
                set_clipboard(contact).await?;
                let search_script = format!(
                    r#"tell application "{app}" to activate
delay 0.5
tell application "System Events"
    keystroke "f" using command down
    delay {search_delay}
    keystroke "v" using command down
    delay {search_delay}
    key code 36
end tell"#,
                    app = escape_applescript(&app_name),
                    search_delay = search_delay
                );
                run_ui_script(search_script).await?;
                tokio::time::sleep(Duration::from_secs_f64(0.6)).await;

                // 3. 粘贴消息 → 回车发送
                set_clipboard(message).await?;
                let send_script = format!(
                    r#"tell application "System Events"
    keystroke "v" using command down
    delay {send_delay}
    key code 36
end tell"#,
                    send_delay = send_delay
                );
                run_ui_script(send_script).await?;

                Ok(serde_json::json!({
                    "success": true,
                    "sent_to": contact,
                    "note": "消息已通过搜索定位会话发送；若联系人关键字命中多条会话，请用更精确的名称重试"
                }))
            }
            _ => Err(format!("微信连接器不支持工具: {tool}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_applescript_strings() {
        assert_eq!(escape_applescript(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn manifest_risks() {
        let conn = WeChatConnector;
        let inst = SoftwareInstance {
            id: 1,
            connector_id: "wechat".into(),
            name: "微信".into(),
            config: serde_json::json!({}),
            mcp_enabled: 1,
            approval_mode: "confirm_high_risk".into(),
            enabled: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let tools = futures_now(conn.list_tools(&inst)).expect("wechat list_tools");
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[3].risk, RiskLevel::HighWrite);
        assert_eq!(tools[0].risk, RiskLevel::ReadOnly);
    }

    #[test]
    fn sanitizes_dumped_lines() {
        let raw = "张三\n\n  李四  \n张三\n很长的行省略\n群聊消息预览";
        let lines = sanitize_dumped_lines(raw);
        assert_eq!(lines, vec!["张三", "李四", "很长的行省略", "群聊消息预览"]);
    }

    // list_tools 是 async trait 方法，测试里同步取结果
    fn futures_now<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }
}
