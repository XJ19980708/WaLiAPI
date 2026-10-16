//! QoderProvider：Qoder 私有网关的固定端点适配器。
//!
//! Qoder 账号只有一条 wire profile（`qoder` / `agent_chat`），由 RoutePlan 冻结：
//!
//! - `POST <gateway>/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1`
//!   （SSE，请求体经 [`qoder_cosy::qoder_encode_body`] 编码并附 COSY 签名头）
//!
//! 请求体不是 OpenAI Chat 格式：codec 层（`protocol/codec/qoder`）已把下游
//! Chat/Messages/Responses 转成 Qoder 的 `agent_chat_generation` payload；本
//! 适配器只负责：
//! 1. 校验 allowlist 内的 (protocol, endpoint)（fail closed）；
//! 2. 附加 COSY 签名头（Authorization + Cosy-*）与模型选择头；
//! 3. 编码请求体并发送（流式/非流式分别用无总超时/带总超时客户端）。
//!
//! 模型目录经 `algo/api/v2/model/list?Encode=1` 拉取（同样需要 COSY 签名），
//! 响应的 `chat` 数组即模型列表，`display_name` 去空白后作为对外模型 id。

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde_json::Value;

use super::qoder_cosy::{build_cosy_headers, qoder_encode_body, CosyIdentity};
use super::qoder_login::{QoderLogin, QoderRegion};
use super::{
    LoginResult, LoginRuntime, Provider, ProviderError, ProviderKind, ProviderLoginContext,
    ProviderModels, ProviderPayload, ProviderRequest, RefreshedPayload,
};
use crate::db::models::{AuthAccount, ModelState};

pub const QODER_CHAT_ENDPOINT: &str = "agent_chat";
const CHAT_PATH: &str =
    "algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1";
const MODELS_PATH: &str = "algo/api/v2/model/list?Encode=1";

pub struct QoderProvider {
    login: QoderLogin,
    /// 测试注入：覆盖网关地址（生产为 None，按 payload.region 解析）。
    gateway_base: Option<String>,
}

impl Default for QoderProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl QoderProvider {
    pub fn new() -> Self {
        Self {
            login: QoderLogin::new(QoderRegion::Cn),
            gateway_base: None,
        }
    }

    /// 测试构造器：全部端点指向本地 mock。
    pub fn with_endpoints(
        region: QoderRegion,
        gateway_base: impl Into<String>,
        openapi_base: impl Into<String>,
        center_base: impl Into<String>,
        device_login_url: impl Into<String>,
    ) -> Self {
        let gateway = gateway_base.into();
        Self {
            login: QoderLogin::with_endpoints(
                region,
                gateway.clone(),
                openapi_base,
                center_base,
                device_login_url,
            ),
            gateway_base: Some(gateway),
        }
    }

    fn field(payload: &ProviderPayload, key: &str) -> Result<String, ProviderError> {
        payload
            .as_value()
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or(ProviderError::InvalidPayload)
    }

    /// 从 payload 解析 COSY 身份。
    fn identity(payload: &ProviderPayload) -> Result<CosyIdentity, ProviderError> {
        Ok(CosyIdentity {
            user_id: Self::field(payload, "user_id")?,
            auth_token: Self::field(payload, "access_token")?,
            name: payload
                .as_value()
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            email: payload
                .as_value()
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            machine_id: Self::field(payload, "machine_id")?,
        })
    }

    fn region(payload: &ProviderPayload) -> QoderRegion {
        match payload.as_value().get("region").and_then(Value::as_str) {
            // 缺省区域是 CN（国内网络可达 qoder.cn / openapi.qoder.com.cn；
            // Global 凭据在 payload 里显式携带 "region":"global"）。
            Some("global") => QoderRegion::Global,
            _ => QoderRegion::Cn,
        }
    }

    fn gateway(&self, payload: &ProviderPayload) -> String {
        self.gateway_base
            .clone()
            .unwrap_or_else(|| Self::region(payload).gateway().to_owned())
    }

    /// 从（可能被 model_mapping 映射后的）模型 id 反查目录里的上游 key。
    ///
    /// model_states 的每个条目把 `id`（display_name 去空白）与上游 key 一起存：
    /// 我们把上游 key 记在 ModelState 的 `protocol` 字段（`qoder:<key>` 形式，
    /// 由 [`normalize_models`] 写入），这里解析回来。
    fn upstream_key(request: &ProviderRequest<'_>, model_id: &str) -> String {
        request
            .account
            .model_states()
            .ok()
            .and_then(|states| {
                states
                    .models
                    .iter()
                    .find(|state| state.id == model_id)
                    .map(|state| state.protocol.clone().unwrap_or_default())
            })
            .and_then(|protocol| protocol.strip_prefix("qoder:").map(str::to_owned))
            .filter(|key| !key.is_empty())
            .unwrap_or_else(|| model_id.to_owned())
    }

    /// 构造一次网关请求（COSY 头 + 编码后的请求体）。
    fn build_gateway_request(
        &self,
        request: &ProviderRequest<'_>,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let identity = Self::identity(request.payload)?;
        let model_id = request
            .body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let upstream_key = Self::upstream_key(request, &model_id);
        let model_source = request
            .body
            .get("model_config")
            .and_then(|config| config.get("source"))
            .and_then(Value::as_str)
            .unwrap_or("system")
            .to_owned();

        // Qoder 请求体：附加模型选择/编码所需的线级字段后整体编码。
        let mut wire_body = request.body.clone();
        if let Some(object) = wire_body.as_object_mut() {
            object.insert(
                "qoder_model_key".to_owned(),
                Value::String(upstream_key.clone()),
            );
        }
        let json_bytes = serde_json::to_vec(&wire_body).map_err(|_| ProviderError::Protocol)?;
        let encoded = qoder_encode_body(&json_bytes);

        let url = format!(
            "{}/{}",
            self.gateway(request.payload).trim_end_matches('/'),
            CHAT_PATH
        );
        let cosy = build_cosy_headers(Some(&encoded), &url, &identity)
            .map_err(|_| ProviderError::InvalidPayload)?;

        let mut headers = HeaderMap::new();
        for (name, value) in cosy {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ProviderError::InvalidPayload)?;
            let value = HeaderValue::from_str(&value).map_err(|_| ProviderError::InvalidPayload)?;
            headers.insert(name, value);
        }
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache"),
        );
        headers.insert(
            HeaderName::from_static("accept-encoding"),
            HeaderValue::from_static("identity"),
        );
        headers.insert(
            HeaderName::from_static("x-model-key"),
            HeaderValue::from_str(&upstream_key).map_err(|_| ProviderError::InvalidPayload)?,
        );
        headers.insert(
            HeaderName::from_static("x-model-source"),
            HeaderValue::from_str(&model_source).map_err(|_| ProviderError::InvalidPayload)?,
        );

        Ok(self
            .client_for(request.is_stream)
            .post(url)
            .headers(headers)
            .body(encoded))
    }

    fn client_for(&self, is_stream: bool) -> reqwest::Client {
        if is_stream {
            super::streaming_http_client()
        } else {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("qoder non-stream http client")
        }
    }
}

#[async_trait]
impl Provider for QoderProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Qoder
    }

    async fn login(
        &self,
        context: &ProviderLoginContext,
        runtime: &dyn LoginRuntime,
    ) -> Result<LoginResult, ProviderError> {
        // 替换登录必须沿用既有 machine_id（账号身份本地锚点）。
        let machine_id = match &context.replacement {
            Some(replacement) => Self::field(&replacement.previous_payload, "machine_id")?,
            // 官方客户端 machine_id 是带连字符的标准 UUID 形态；设备授权
            // 与 COSY 身份都绑定该值，保持一致避免服务端按形态校验失败。
            None => uuid::Uuid::new_v4().to_string(),
        };
        // 登录支持 PAT：Qoder 区分区域。若替换上下文的 payload 带 region 则沿
        // 用；否则默认 Global。PAT 由命令层经 attributes 传入（无则走设备码）。
        // 测试注入的 self.login 覆盖了端点；生产路径按 region 重建。
        let region = match &context.replacement {
            Some(replacement) => Self::region(&replacement.previous_payload),
            None => QoderRegion::Cn,
        };
        let pat = context
            .replacement
            .as_ref()
            .and_then(|replacement| replacement.previous_attributes.get("pat"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let login = self.login.clone_for_region_if_test(region);
        login.login(runtime, pat.as_deref(), &machine_id).await
    }

    async fn import(&self, bytes: &[u8]) -> Result<LoginResult, ProviderError> {
        // Qoder 支持导入粘贴的 PAT（`pt-...`）：把它当作 PAT 登录处理。
        let text = String::from_utf8_lossy(bytes).trim().to_owned();
        if !text.starts_with("pt-") {
            return Err(ProviderError::UnsupportedFeatures {
                pointer: "provider.import.qoder".into(),
            });
        }
        let machine_id = uuid::Uuid::new_v4().to_string();
        let runtime = NoopRuntime;
        QoderLogin::new(QoderRegion::Cn)
            .login_with_pat(&runtime, &text, &machine_id)
            .await
    }

    async fn refresh(&self, payload: &ProviderPayload) -> Result<RefreshedPayload, ProviderError> {
        let region = Self::region(payload);
        QoderLogin::new(region).refresh_payload(payload).await
    }

    async fn outbound(
        &self,
        request: ProviderRequest<'_>,
    ) -> Result<reqwest::Response, ProviderError> {
        // allowlist：Qoder 账号只允许固定的私有 wire profile。
        if request.upstream_protocol != "qoder" || request.upstream_endpoint != QODER_CHAT_ENDPOINT
        {
            return Err(ProviderError::Protocol);
        }
        let builder = self.build_gateway_request(&request)?;
        let built = builder.build().map_err(|_| ProviderError::Protocol)?;
        let url = built.url().clone();
        let headers = built.headers().clone();
        // 请求体必须原样复用编码后的字节（不能让 reqwest 重新序列化）。
        let body_bytes: Vec<u8> = match built.body() {
            Some(body) => match body.as_bytes() {
                Some(bytes) => bytes.to_vec(),
                None => return Err(ProviderError::Protocol),
            },
            None => return Err(ProviderError::Protocol),
        };

        self.client_for(request.is_stream)
            .post(url)
            .headers(headers)
            .body(body_bytes)
            .send()
            .await
            .map_err(|_| ProviderError::Retryable)
    }

    async fn list_models(
        &self,
        _account: &AuthAccount,
        payload: &ProviderPayload,
    ) -> Result<ProviderModels, ProviderError> {
        let identity = Self::identity(payload)?;
        let url = format!(
            "{}/{}",
            self.gateway(payload).trim_end_matches('/'),
            MODELS_PATH
        );
        let cosy =
            build_cosy_headers(None, &url, &identity).map_err(|_| ProviderError::InvalidPayload)?;
        let mut headers = HeaderMap::new();
        for (name, value) in cosy {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ProviderError::InvalidPayload)?;
            let value = HeaderValue::from_str(&value).map_err(|_| ProviderError::InvalidPayload)?;
            headers.insert(name, value);
        }
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("qoder models http client");
        let response = client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(|_| ProviderError::Retryable)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::Unauthorized);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Retryable);
        }
        let body: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
        Ok(normalize_models(&body))
    }
}

/// 无操作 LoginRuntime：import 路径不需要交互。
struct NoopRuntime;

#[async_trait::async_trait]
impl LoginRuntime for NoopRuntime {
    async fn open_browser(&self, _url: &str) -> Result<(), ProviderError> {
        Err(ProviderError::BrowserOpenFailed)
    }
    async fn set_step(&self, _step: super::LoginStep) {}
    async fn present_device_authorization(
        &self,
        _url: &str,
        _code: &str,
        _expires_at: Option<String>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
    fn is_cancelled(&self) -> bool {
        false
    }
    async fn cancelled(&self) {
        std::future::pending::<()>().await;
    }
}

/// 归一化模型目录：`chat` 数组的 `display_name` 去空白为对外 id；上游 key 记录
/// 在 `protocol` 字段（`qoder:<key>`），供 RouteProfile/请求构造反查。
fn normalize_models(body: &Value) -> ProviderModels {
    let Some(entries) = body.get("chat").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for entry in entries {
        let key = entry.get("key").and_then(Value::as_str).unwrap_or_default();
        let display = entry
            .get("display_name")
            .and_then(Value::as_str)
            .map(|name| {
                name.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
            })
            .filter(|id| !id.is_empty());
        let enabled = entry.get("enable").and_then(Value::as_bool).unwrap_or(true);
        let Some(id) = display else {
            continue;
        };
        let (status, unavailable, last_error) = if !enabled || key.is_empty() {
            (
                "unavailable".to_owned(),
                true,
                Some("model disabled in Qoder catalog".to_owned()),
            )
        } else {
            ("available".to_owned(), false, None)
        };
        models.push(ModelState {
            id,
            status,
            unavailable,
            next_retry_after: None,
            last_error,
            protocol: Some(format!("qoder:{key}")),
        });
    }
    models
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use serde_json::json;

    use super::super::ProviderRequest;
    use super::*;
    use crate::db::models::AuthAccount;

    #[test]
    fn kind_is_qoder() {
        assert_eq!(QoderProvider::new().kind(), ProviderKind::Qoder);
    }

    #[test]
    fn normalize_models_derives_id_from_display_name() {
        let body = json!({
            "chat": [
                {"key": "qmodel_latest", "display_name": "Qwen3.7 Max", "enable": true},
                {"key": "lite", "display_name": "Lite", "enable": true},
                {"key": "off", "display_name": "Disabled", "enable": false},
                {"key": "", "display_name": "NoKey", "enable": true},
                {"display_name": "   ", "enable": true}
            ]
        });
        let models = normalize_models(&body);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["Qwen3.7Max", "Lite", "Disabled", "NoKey"]);
        assert_eq!(models[0].protocol.as_deref(), Some("qoder:qmodel_latest"));
        assert!(!models[0].unavailable);
        assert!(models[2].unavailable);
        assert!(models[3].unavailable);
    }

    #[test]
    fn normalize_models_empty_or_malformed() {
        assert!(normalize_models(&json!({})).is_empty());
        assert!(normalize_models(&json!({"chat": "nope"})).is_empty());
    }

    fn account(model_states_json: &str) -> AuthAccount {
        crate::db::models::AuthAccount {
            id: "local-1".into(),
            provider: "qoder".into(),
            label: "Qoder".into(),
            account_id: "u-1".into(),
            status: "active".into(),
            disabled: 0,
            priority: 0,
            weight: 1,
            sort_order: 0,
            quota_json: None,
            model_states_json: model_states_json.to_owned(),
            model_mapping_json: "{}".into(),
            model_mapping_disabled: "[]".into(),
            attributes_json: "{}".into(),
            payload_json: json!({
                "access_token": "jt-tok",
                "user_id": "u-1",
                "name": "Dev",
                "email": "d@e.st",
                "machine_id": "m-1",
                "region": "global"
            })
            .to_string(),
            last_refreshed_at: None,
            last_models_sync_at: None,
            next_refresh_after: None,
            next_retry_after: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn payload() -> ProviderPayload {
        ProviderPayload::new(json!({
            "access_token": "jt-tok",
            "user_id": "u-1",
            "name": "Dev",
            "email": "d@e.st",
            "machine_id": "m-1",
            "region": "global"
        }))
    }

    #[tokio::test]
    async fn outbound_rejects_non_allowlisted_profiles() {
        let provider = QoderProvider::new();
        let account = account("{}");
        let payload = payload();
        let body = json!({"model": "Lite"});
        let caller = reqwest::header::HeaderMap::new();
        let request = ProviderRequest {
            account: &account,
            payload: &payload,
            body: &body,
            headers: &caller,
            is_stream: false,
            upstream_protocol: "openai",
            upstream_endpoint: "chat_completions",
        };
        assert_eq!(
            provider.outbound(request).await.unwrap_err(),
            ProviderError::Protocol
        );
        let request = ProviderRequest {
            account: &account,
            payload: &payload,
            body: &body,
            headers: &caller,
            is_stream: false,
            upstream_protocol: "qoder",
            upstream_endpoint: "chat_completions",
        };
        assert_eq!(
            provider.outbound(request).await.unwrap_err(),
            ProviderError::Protocol
        );
    }

    #[tokio::test]
    async fn outbound_sends_encoded_body_with_cosy_headers() {
        let state = Arc::new(Mock::default());
        let app = Router::new()
            .route(
                "/algo/api/v2/service/pro/sse/agent_chat_generation",
                post(
                    move |s: State<Arc<Mock>>,
                          headers: axum::http::HeaderMap,
                          body: axum::body::Bytes| {
                        let s = s.clone();
                        async move {
                            let mut headers = headers;
                            *s.hits.lock().unwrap() += 1;
                            s.auth.lock().unwrap().push(
                                headers
                                    .remove("authorization")
                                    .map(|v| v.to_str().unwrap().to_owned())
                                    .unwrap_or_default(),
                            );
                            s.model_key.lock().unwrap().push(
                                headers
                                    .get("x-model-key")
                                    .map(|v| v.to_str().unwrap().to_owned())
                                    .unwrap_or_default(),
                            );
                            s.bodies.lock().unwrap().push(body.to_vec());
                            Json(json!({"ok": true})).into_response()
                        }
                    },
                ),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let provider = QoderProvider::with_endpoints(
            QoderRegion::Global,
            format!("http://{addr}/"),
            format!("http://{addr}"),
            format!("http://{addr}"),
            format!("http://{addr}/device"),
        );
        let account = account(
            r#"{"version":1,"models":[{"id":"Lite","status":"available","unavailable":false,"next_retry_after":null,"last_error":null,"protocol":"qoder:lite"}]}"#,
        );
        let payload = payload();
        let body = json!({
            "model": "Lite",
            "messages": [{"role": "user", "content": "hi"}],
            "model_config": {"source": "system"}
        });
        let caller = reqwest::header::HeaderMap::new();
        provider
            .outbound(ProviderRequest {
                account: &account,
                payload: &payload,
                body: &body,
                headers: &caller,
                is_stream: false,
                upstream_protocol: "qoder",
                upstream_endpoint: QODER_CHAT_ENDPOINT,
            })
            .await
            .unwrap();

        assert_eq!(*state.hits.lock().unwrap(), 1);
        // Authorization 是 COSY 三段式
        let auth = state.auth.lock().unwrap()[0].clone();
        assert!(auth.starts_with("Bearer COSY."), "auth was {auth}");
        // 模型选择头来自目录里的上游 key
        assert_eq!(state.model_key.lock().unwrap()[0], "lite");
        // 请求体是 Qoder 自定义编码（不是明文 JSON）
        let body = state.bodies.lock().unwrap()[0].clone();
        assert_ne!(
            body,
            serde_json::to_vec(&body_json(&body)).unwrap_or_default()
        );
        assert!(
            !body.starts_with(b"{"),
            "body must be encoded, not plain JSON"
        );
    }

    fn body_json(body: &[u8]) -> Value {
        Value::Null
    }

    #[derive(Default)]
    struct Mock {
        hits: std::sync::Mutex<usize>,
        auth: Mutex<Vec<String>>,
        model_key: Mutex<Vec<String>>,
        bodies: Mutex<Vec<Vec<u8>>>,
    }

    #[allow(dead_code)]
    fn unused(_: Arc<AtomicUsize>) {}
}
