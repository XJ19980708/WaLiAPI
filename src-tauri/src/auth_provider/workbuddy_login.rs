//! WorkBuddy web-handoff login and token refresh.

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;

use super::{
    LoginResult, LoginRuntime, LoginStep, ProviderError, ProviderPayload, RefreshedPayload,
};

pub const WORKBUDDY_API_BASE: &str = "https://copilot.tencent.com";
pub const WORKBUDDY_WEBSITE_BASE: &str = "https://www.workbuddy.cn";
pub const WORKBUDDY_LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const WORKBUDDY_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
pub const WORKBUDDY_POLL_INTERVAL: u64 = 1;
const WORKBUDDY_USER_AGENT: &str = "WorkBuddy/5.8.2 WaLiAPI";
const WORKBUDDY_PLUGIN_VERSION: &str = "5.8.2";

const AUTH_STATE_PATH: &str = "/v2/plugin/auth/state?platform=workbuddy";
const AUTH_TOKEN_PATH: &str = "/v2/plugin/auth/token";
const LOGIN_ACCOUNT_PATH: &str = "/v2/plugin/login/account";
const TOKEN_REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";

#[derive(Clone)]
pub struct WorkBuddyLogin {
    client: reqwest::Client,
    api_base: String,
}

impl Default for WorkBuddyLogin {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkBuddyLogin {
    pub fn new() -> Self {
        Self::with_endpoints(WORKBUDDY_API_BASE, WORKBUDDY_WEBSITE_BASE)
    }

    pub fn with_endpoints(api_base: impl Into<String>, _website_base: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(WORKBUDDY_HTTP_TIMEOUT)
                .build()
                .expect("workbuddy login http client"),
            api_base: api_base.into().trim_end_matches('/').to_owned(),
        }
    }

    pub async fn login(&self, runtime: &dyn LoginRuntime) -> Result<LoginResult, ProviderError> {
        let deadline = tokio::time::Instant::now() + WORKBUDDY_LOGIN_TIMEOUT;
        runtime.set_step(LoginStep::Preparing).await;
        let state = self.request_auth_state().await?;
        if state.state.is_empty() || state.auth_url.is_empty() {
            return Err(ProviderError::DeviceAuthorizationFailed);
        }
        runtime.set_step(LoginStep::Authorizing).await;
        let auth_url = browser_auth_url(&state.auth_url);
        runtime
            .present_device_authorization(&auth_url, "WorkBuddy", None)
            .await?;
        let _ = runtime.open_browser(&auth_url).await;

        let tokens = self
            .poll_auth_token(runtime, deadline, &state.state)
            .await?;
        runtime.set_step(LoginStep::Exchanging).await;
        let account = WorkBuddyAccount::default();
        runtime.set_step(LoginStep::Saving).await;
        let user_id = account.user_id.clone().unwrap_or_else(|| "unknown".into());
        let payload = json_payload(&tokens, &account);
        Ok(LoginResult {
            account_id: user_id,
            label: account.display_name.unwrap_or_else(|| "WorkBuddy".into()),
            attributes: serde_json::json!({
                "account_type": account.account_type,
                "enterprise_id": account.enterprise_id,
                "domain": tokens.raw.get("domain").cloned().unwrap_or(Value::Null),
                "login_mode": "web_handoff",
            }),
            payload,
            last_refreshed_at: Some(Utc::now().to_rfc3339()),
            next_refresh_after: None,
            next_retry_after: None,
        })
    }

    async fn request_auth_state(&self) -> Result<AuthState, ProviderError> {
        let response = self
            .client
            .post(format!("{}{AUTH_STATE_PATH}", self.api_base))
            .headers(no_auth_headers())
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|_| ProviderError::Retryable)?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
        let code = body.get("code").and_then(Value::as_i64);
        tracing::info!(target: "workbuddy_login", status = %status, code, "workbuddy auth state response");
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::Unauthorized);
        }
        if !status.is_success()
            || body
                .get("code")
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0)
        {
            return Err(ProviderError::LoginFailed);
        }
        Ok(AuthState {
            state: body
                .pointer("/data/state")
                .or_else(|| body.pointer("/state"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            auth_url: body
                .pointer("/data/authUrl")
                .or_else(|| body.pointer("/authUrl"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }

    async fn poll_auth_token(
        &self,
        runtime: &dyn LoginRuntime,
        deadline: tokio::time::Instant,
        state: &str,
    ) -> Result<AuthTokens, ProviderError> {
        loop {
            if runtime.is_cancelled() {
                return Err(ProviderError::LoginCancelled);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ProviderError::LoginTimeout);
            }
            tokio::time::sleep(Duration::from_secs(WORKBUDDY_POLL_INTERVAL)).await;
            let response = self
                .client
                .get(format!("{}{AUTH_TOKEN_PATH}", self.api_base))
                .query(&[("state", state)])
                .headers(no_auth_headers())
                .send()
                .await
                .map_err(|_| ProviderError::Retryable)?;
            let status = response.status();
            let body: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
            let code = body.get("code").and_then(Value::as_i64);
            let has_token = body
                .pointer("/data/accessToken")
                .and_then(Value::as_str)
                .is_some();
            tracing::info!(target: "workbuddy_login", status = %status, code, has_token, "workbuddy token poll response");
            if status.is_success() {
                if let Some(tokens) = AuthTokens::from_value(body.pointer("/data")) {
                    if tokens.usable() {
                        return Ok(tokens);
                    }
                }
                if body.get("code").and_then(Value::as_i64) == Some(11217) {
                    continue;
                }
                return Err(ProviderError::TokenExchangeFailed);
            }
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::Unauthorized);
            }
            return Err(ProviderError::Retryable);
        }
    }

    async fn poll_account(
        &self,
        runtime: &dyn LoginRuntime,
        deadline: tokio::time::Instant,
        state: &str,
        tokens: &AuthTokens,
    ) -> Result<WorkBuddyAccount, ProviderError> {
        loop {
            if runtime.is_cancelled() {
                return Err(ProviderError::LoginCancelled);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ProviderError::LoginTimeout);
            }
            tokio::time::sleep(Duration::from_secs(WORKBUDDY_POLL_INTERVAL)).await;
            runtime.cancelled().await;
            let response = self
                .client
                .get(format!("{}{LOGIN_ACCOUNT_PATH}", self.api_base))
                .query(&[("state", state)])
                .headers(auth_headers(tokens, None))
                .send()
                .await
                .map_err(|_| ProviderError::Retryable)?;
            let status = response.status();
            let body: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
            if status.is_success() {
                let account = WorkBuddyAccount::from_value(body.pointer("/data"));
                if account.user_id.is_some() {
                    return Ok(account);
                }
                if body.get("code").and_then(Value::as_i64) == Some(12151) {
                    continue;
                }
                return Err(ProviderError::LoginFailed);
            }
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::Unauthorized);
            }
            return Err(ProviderError::Retryable);
        }
    }

    pub async fn refresh_payload(
        &self,
        payload: &ProviderPayload,
    ) -> Result<RefreshedPayload, ProviderError> {
        let refresh_token = payload
            .as_value()
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ProviderError::InvalidPayload)?;
        let mut headers = basic_headers();
        headers.insert(
            HeaderName::from_static("x-refresh-token"),
            HeaderValue::from_str(refresh_token).map_err(|_| ProviderError::InvalidPayload)?,
        );
        headers.insert(
            HeaderName::from_static("x-auth-refresh-source"),
            HeaderValue::from_static("plugin"),
        );
        let response = self
            .client
            .post(format!("{}{TOKEN_REFRESH_PATH}", self.api_base))
            .headers(headers)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|_| ProviderError::Retryable)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::Unauthorized);
        }
        if !status.is_success() {
            return Err(ProviderError::Retryable);
        }
        let body: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
        let Some(tokens) = AuthTokens::from_value(body.pointer("/data")) else {
            return Err(ProviderError::Protocol);
        };
        if !tokens.usable() {
            return Err(ProviderError::Protocol);
        }
        let mut next = tokens.raw.clone();
        if next.get("refresh_token").and_then(Value::as_str).is_none() {
            next["refresh_token"] = Value::String(refresh_token.to_owned());
        }
        for key in [
            "user_id",
            "account_type",
            "enterprise_id",
            "domain",
            "client_version",
        ] {
            if let Some(value) = payload.as_value().get(key) {
                next[key] = value.clone();
            }
        }
        Ok(RefreshedPayload {
            payload: ProviderPayload::new(next),
            last_refreshed_at: Some(Utc::now().to_rfc3339()),
            next_refresh_after: None,
            next_retry_after: None,
        })
    }

    pub fn api_base(&self) -> &str {
        &self.api_base
    }
}

#[derive(Clone, Debug, Default)]
struct AuthTokens {
    raw: Value,
}

#[derive(Clone, Debug)]
struct AuthState {
    state: String,
    auth_url: String,
}

impl AuthTokens {
    fn from_value(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        let mut raw = value.clone();
        if raw.get("access_token").and_then(Value::as_str).is_none() {
            if let Some(token) = raw.get("accessToken").and_then(Value::as_str) {
                raw["access_token"] = Value::String(token.to_owned());
            }
        }
        if raw.get("refresh_token").and_then(Value::as_str).is_none() {
            if let Some(token) = raw.get("refreshToken").and_then(Value::as_str) {
                raw["refresh_token"] = Value::String(token.to_owned());
            }
        }
        if raw.get("expiresAt").is_none() {
            if let Some(expires_in) = raw.get("expiresIn").and_then(Value::as_i64) {
                raw["expiresAt"] = Value::Number(
                    (Utc::now().timestamp_millis() + expires_in.saturating_mul(1000)).into(),
                );
            }
        }
        if raw.get("refreshExpiresAt").is_none() {
            if let Some(expires_in) = raw.get("refreshExpiresIn").and_then(Value::as_i64) {
                raw["refreshExpiresAt"] = Value::Number(
                    (Utc::now().timestamp_millis() + expires_in.saturating_mul(1000)).into(),
                );
            }
        }
        let usable = raw
            .get("access_token")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        usable.then_some(Self { raw })
    }

    fn usable(&self) -> bool {
        !self.access_token().unwrap_or_default().is_empty()
    }

    fn access_token(&self) -> Option<&str> {
        self.raw.get("access_token").and_then(Value::as_str)
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkBuddyAccount {
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    pub account_type: Option<String>,
    pub enterprise_id: Option<String>,
}

impl WorkBuddyAccount {
    fn from_value(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        Self {
            user_id: first_string(value, &["uid", "user_id", "userId", "id"]),
            display_name: first_string(
                value,
                &["nickname", "displayName", "display_name", "email"],
            ),
            account_type: first_string(value, &["type", "accountType", "account_type"]),
            enterprise_id: first_string(value, &["enterpriseId", "enterprise_id"]),
        }
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|item| !item.trim().is_empty())
            .map(str::to_owned)
    })
}

fn json_payload(tokens: &AuthTokens, account: &WorkBuddyAccount) -> ProviderPayload {
    let mut value = tokens.raw.clone();
    if let Some(user_id) = &account.user_id {
        value["user_id"] = Value::String(user_id.clone());
    }
    if let Some(account_type) = &account.account_type {
        value["account_type"] = Value::String(account_type.clone());
    }
    if let Some(enterprise_id) = &account.enterprise_id {
        value["enterprise_id"] = Value::String(enterprise_id.clone());
    }
    if let Some(expires_at) = expires_at_from(&value) {
        value["expires_at"] = Value::String(expires_at);
    }
    ProviderPayload::new(value)
}

fn expires_at_from(value: &Value) -> Option<String> {
    let value = value.get("expires_at").or_else(|| value.get("expiresAt"))?;
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let millis = value.as_i64()?;
    DateTime::from_timestamp_millis(millis).map(|time| time.with_timezone(&Utc).to_rfc3339())
}

fn basic_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static(WORKBUDDY_USER_AGENT));
    headers
}

fn no_auth_headers() -> HeaderMap {
    let mut headers = basic_headers();
    headers.insert(
        HeaderName::from_static("x-no-authorization"),
        HeaderValue::from_static("true"),
    );
    headers.insert(
        HeaderName::from_static("x-no-user-id"),
        HeaderValue::from_static("true"),
    );
    headers.insert(
        HeaderName::from_static("x-no-enterprise-id"),
        HeaderValue::from_static("true"),
    );
    headers.insert(
        HeaderName::from_static("x-no-department-info"),
        HeaderValue::from_static("true"),
    );
    headers
}

fn browser_auth_url(auth_url: &str) -> String {
    match url::Url::parse(auth_url) {
        Ok(mut url) => {
            url.query_pairs_mut()
                .append_pair("version", WORKBUDDY_PLUGIN_VERSION);
            url.to_string()
        }
        Err(_) => auth_url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
    use serde_json::json;

    use super::*;

    struct CancelledRuntime {
        after: usize,
        polls: Arc<AtomicUsize>,
    }

    struct CancelledBeforePollRuntime;

    impl CancelledRuntime {
        fn never() -> (Self, Arc<AtomicUsize>) {
            let polls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    after: usize::MAX,
                    polls: polls.clone(),
                },
                polls,
            )
        }
    }

    #[async_trait::async_trait]
    impl LoginRuntime for CancelledRuntime {
        async fn open_browser(&self, _: &str) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn set_step(&self, _: LoginStep) {}

        async fn present_device_authorization(
            &self,
            _: &str,
            _: &str,
            _: Option<String>,
        ) -> Result<(), ProviderError> {
            Ok(())
        }

        fn is_cancelled(&self) -> bool {
            self.polls.load(Ordering::SeqCst) >= self.after
        }

        async fn cancelled(&self) {}
    }

    #[async_trait::async_trait]
    impl LoginRuntime for CancelledBeforePollRuntime {
        async fn open_browser(&self, _: &str) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn set_step(&self, _: LoginStep) {}

        async fn present_device_authorization(
            &self,
            _: &str,
            _: &str,
            _: Option<String>,
        ) -> Result<(), ProviderError> {
            Ok(())
        }

        fn is_cancelled(&self) -> bool {
            true
        }

        async fn cancelled(&self) {}
    }

    fn auth_payload() -> ProviderPayload {
        ProviderPayload::new(json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "user_id": "user-1",
            "enterprise_id": "tenant-1",
            "domain": "example.tencent.com",
            "client_version": "5.8.2",
            "expires_at": 1_900_000_000_000i64,
        }))
    }

    async fn start_router(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn refresh_preserves_refresh_token_when_response_omits_it() {
        let app = Router::new().route(
            TOKEN_REFRESH_PATH,
            post(
                |headers: reqwest::header::HeaderMap, Json(body): Json<Value>| async move {
                    assert!(headers.contains_key("x-refresh-token"));
                    assert_eq!(body, serde_json::json!({}));
                    Json(json!({
                        "code": 0,
                        "data": {"access_token": "next-access", "expires_at": "2030-01-01T00:00:00Z"}
                    }))
                },
            ),
        );
        let base = start_router(app).await;
        let login = WorkBuddyLogin::with_endpoints(base, WORKBUDDY_WEBSITE_BASE);
        let refreshed = login.refresh_payload(&auth_payload()).await.unwrap();
        assert_eq!(refreshed.payload.as_value()["access_token"], "next-access");
        assert_eq!(
            refreshed.payload.as_value()["refresh_token"],
            "refresh-token"
        );
        assert_eq!(refreshed.payload.as_value()["user_id"], "user-1");
        assert_eq!(
            refreshed.payload.as_value()["expires_at"],
            "2030-01-01T00:00:00Z"
        );
    }

    #[tokio::test]
    async fn auth_state_returns_server_provided_auth_url() {
        let app = Router::new().route(
            "/v2/plugin/auth/state",
            post(|| async {
                Json(json!({
                    "code": 0,
                    "data": {
                        "state": "state-1",
                        "authUrl": "https://www.workbuddy.cn/login?platform=workbuddy&state=state-1"
                    }
                }))
            }),
        );
        let base = start_router(app).await;
        let login = WorkBuddyLogin::with_endpoints(base, WORKBUDDY_WEBSITE_BASE);
        let state = login.request_auth_state().await.unwrap();
        assert_eq!(state.state, "state-1");
        assert_eq!(
            state.auth_url,
            "https://www.workbuddy.cn/login?platform=workbuddy&state=state-1"
        );
    }

    #[tokio::test]
    async fn auth_token_polling_success_and_pending() {
        let polls = Arc::new(AtomicUsize::new(0));
        let poll_count = polls.clone();
        let app = Router::new()
            .route(
                AUTH_TOKEN_PATH,
                get(move || {
                    let poll_count = poll_count.clone();
                    async move {
                        let poll = poll_count.fetch_add(1, Ordering::SeqCst);
                        if poll == 0 {
                            Json(json!({"code": 11217, "message": "pending"}))
                        } else {
                            Json(json!({
                                "code": 0,
                                "data": {
                                    "accessToken": "access-token",
                                    "refreshToken": "refresh-token",
                                    "domain": "www.workbuddy.cn"
                                }
                            }))
                        }
                    }
                }),
            )
            .route(
                LOGIN_ACCOUNT_PATH,
                get(|| async {
                    Json(json!({
                        "code": 0,
                        "data": {"uid": "user-1", "nickname": "User", "enterpriseId": "tenant-1"}
                    }))
                }),
            );
        let base = start_router(app).await;
        let login = WorkBuddyLogin::with_endpoints(base, WORKBUDDY_WEBSITE_BASE);
        let (runtime, _) = CancelledRuntime::never();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let tokens = login
            .poll_auth_token(&runtime, deadline, "state-1")
            .await
            .unwrap();
        assert_eq!(tokens.access_token(), Some("access-token"));
        assert_eq!(tokens.raw["refresh_token"], "refresh-token");
        assert_eq!(tokens.raw["domain"], "www.workbuddy.cn");
    }

    #[tokio::test]
    async fn auth_token_polling_cancels_before_next_request() {
        let app = Router::new().route(
            AUTH_TOKEN_PATH,
            get(|| async { Json(json!({"code": 11217, "message": "pending"})) }),
        );
        let base = start_router(app).await;
        let login = WorkBuddyLogin::with_endpoints(base, WORKBUDDY_WEBSITE_BASE);
        let runtime = CancelledBeforePollRuntime;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let error = login
            .poll_auth_token(&runtime, deadline, "state-1")
            .await
            .unwrap_err();
        assert_eq!(error, ProviderError::LoginCancelled);
    }

    #[test]
    fn domain_is_read_from_raw_payload() {
        let tokens = AuthTokens::from_value(Some(&json!({
            "access_token": "access-token",
            "domain": "example.tencent.com"
        })))
        .unwrap();
        let headers = auth_headers(&tokens, None);
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer access-token");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
    }

    #[test]
    fn browser_auth_url_appends_plugin_version_once() {
        assert_eq!(
            browser_auth_url("https://www.workbuddy.cn/login?platform=workbuddy&state=state-1"),
            "https://www.workbuddy.cn/login?platform=workbuddy&state=state-1&version=5.8.2"
        );
    }
}

fn auth_headers(tokens: &AuthTokens, account: Option<&WorkBuddyAccount>) -> HeaderMap {
    let mut headers = basic_headers();
    if let Some(token) = tokens.access_token() {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
    }
    if let Some(domain) = tokens.raw.get("domain").and_then(Value::as_str) {
        if let Ok(value) = HeaderValue::from_str(domain) {
            headers.insert(HeaderName::from_static("x-domain"), value);
        }
    }
    headers.insert(
        HeaderName::from_static("x-no-user-id"),
        HeaderValue::from_static("true"),
    );
    headers.insert(
        HeaderName::from_static("x-no-enterprise-id"),
        HeaderValue::from_static("true"),
    );
    headers.insert(
        HeaderName::from_static("x-no-department-info"),
        HeaderValue::from_static("true"),
    );
    let account = account.cloned().unwrap_or_default();
    let user_id = account.user_id.unwrap_or_default();
    if !user_id.is_empty() {
        headers.insert(
            HeaderName::from_static("x-user-id"),
            HeaderValue::from_str(&user_id).unwrap_or(HeaderValue::from_static("unknown")),
        );
    }
    if let Some(enterprise_id) = account.enterprise_id {
        if let Ok(value) = HeaderValue::from_str(&enterprise_id) {
            headers.insert(HeaderName::from_static("x-enterprise-id"), value.clone());
            headers.insert(HeaderName::from_static("x-tenant-id"), value);
        }
    }
    headers
}
