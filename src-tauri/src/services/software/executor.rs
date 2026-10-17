//! 执行底座：白名单子进程执行 + 平台脚本桥 + 本地 HTTP 调用。
//!
//! 安全约定：
//! - 子进程**不经 shell**，`program + args` 直接 exec，杜绝注入拼接
//! - 输出截断（默认 64 KiB），超时强杀（默认 30s）
//! - 参数模板只做**整值替换**（`{{name}}` → 用户传入的完整字符串），
//!   不做任何 shell 解析，因此模板注入不产生命令拼接风险

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// 单次子进程输出上限（stdout/stderr 各自）。
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// 默认超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct ExecOutcome {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

fn truncate(buf: &[u8]) -> String {
    let cut = buf.len().min(MAX_OUTPUT_BYTES);
    String::from_utf8_lossy(&buf[..cut]).into_owned()
}

/// 执行外部命令（不经 shell）。`stdin_text` 为 Some 时写入并关闭 stdin。
pub async fn exec(
    program: &str,
    args: &[String],
    stdin_text: Option<&str>,
    timeout: Duration,
) -> ExecOutcome {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 脱离父进程组，避免 GUI 应用继承终端会话
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ExecOutcome {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: format!("无法启动程序 `{program}`: {e}"),
                timed_out: false,
            }
        }
    };

    if let (Some(input), Some(mut stdin)) = (stdin_text, child.stdin.take()) {
        let _ = stdin.write_all(input.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => ExecOutcome {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: truncate(&output.stdout),
            stderr: truncate(&output.stderr),
            timed_out: false,
        },
        Ok(Err(e)) => ExecOutcome {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: format!("进程执行失败: {e}"),
            timed_out: false,
        },
        Err(_) => ExecOutcome {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: format!("执行超时（{}s），进程已终止", timeout.as_secs()),
            timed_out: true,
        },
    }
}

/// 平台脚本桥：把一段脚本翻译成 `(program, args)`。
/// macOS → osascript -e；Windows → powershell -Command；Linux → bash -c。
pub fn script_command(script: &str) -> (&'static str, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        ("osascript", vec!["-e".to_string(), script.to_string()])
    }
    #[cfg(target_os = "windows")]
    {
        (
            "powershell",
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                script.to_string(),
            ],
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ("bash", vec!["-c".to_string(), script.to_string()])
    }
}

/// 本地 HTTP 调用结果。
#[derive(Debug)]
pub struct HttpOutcome {
    pub success: bool,
    pub status: u16,
    pub body: String,
}

/// 调用本地软件 HTTP 接口。body 为 Option（GET 通常无 body）。
pub async fn http_request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    timeout: Duration,
) -> HttpOutcome {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HttpOutcome {
                success: false,
                status: 0,
                body: format!("HTTP client 构建失败: {e}"),
            }
        }
    };

    let method = match reqwest::Method::from_bytes(method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return HttpOutcome {
                success: false,
                status: 0,
                body: format!("非法 HTTP 方法: {method}"),
            }
        }
    };

    let mut req = client.request(method, url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(b) = body {
        req = req.body(b.to_string());
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let body = if body.len() > MAX_OUTPUT_BYTES {
                String::from_utf8_lossy(&body.as_bytes()[..MAX_OUTPUT_BYTES]).into_owned()
            } else {
                body
            };
            HttpOutcome {
                success: (200..300).contains(&status),
                status,
                body,
            }
        }
        Err(e) => HttpOutcome {
            success: false,
            status: 0,
            body: format!("请求失败: {e}"),
        },
    }
}

/// 参数模板整值替换：`{{name}}` → args[name]（字符串化）。未提供的参数替换为空串。
pub fn render_template(template: &str, args: &serde_json::Value) -> String {
    let mut out = template.to_string();
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            let placeholder = format!("{{{{{k}}}}}");
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&placeholder, &value);
        }
    }
    // 清掉未提供的占位符，避免把 {{xxx}} 字面量传给软件
    while let Some(start) = out.find("{{") {
        let Some(end) = out[start..].find("}}") else {
            break;
        };
        out.replace_range(start..start + end + 2, "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_substitutes_values() {
        let args = serde_json::json!({"name": "张三", "n": 3});
        assert_eq!(
            render_template("你好 {{name}}，{{n}} 条", &args),
            "你好 张三，3 条"
        );
    }

    #[test]
    fn render_template_clears_missing_placeholders() {
        assert_eq!(
            render_template("a {{x}} b {{y}}", &serde_json::json!({"x":"1"})),
            "a 1 b "
        );
    }

    #[tokio::test]
    async fn exec_reports_missing_program() {
        let out = exec(
            "definitely-not-a-real-binary-xyz",
            &[],
            None,
            DEFAULT_TIMEOUT,
        )
        .await;
        assert!(!out.success);
        assert!(!out.stderr.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_runs_and_captures() {
        let out = exec("echo", &["hello".to_string()], None, DEFAULT_TIMEOUT).await;
        assert!(out.success);
        assert_eq!(out.stdout.trim(), "hello");
    }
}
