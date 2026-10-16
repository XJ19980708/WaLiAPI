//! Qoder 登录：浏览器设备码流程（Global 区）+ PAT 交换（Global/CN）+ token 刷新。
//!
//! Qoder 没有标准 OAuth 2.0 端点，登录协议是 Qoder CLI 私有的：
//!
//! - **设备码**：生成 PKCE verifier/challenge + nonce，打开
//!   `https://qoder.com/device/selectAccounts?challenge=...&nonce=...`，
//!   然后轮询 `openapi.qoder.sh/api/v1/deviceToken/poll?nonce=..&verifier=..`，
//!   `202/404` 表示等待授权，`200` 返回 job token。
//! - **PAT**：`pt-` 开头的个人令牌经
//!   `POST <openapi>/api/v1/jobToken/exchange {"personal_token": ...}`
//!   换成短时 job token（无需 COSY 签名），再经 userinfo 解析身份。
//!   这是 CN 区（`qoder.com.cn`）唯一可用的登录方式。
//! - **刷新**：`POST <center>/algo/api/v3/user/refresh_token {"refreshToken": ...}`，
//!   Bearer 携带当前 access token。
//!
//! 本模块只负责登录/刷新/HTTP 状态分类，不做协议转换。

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};

use super::qoder_cosy::{
    code_challenge_s256, generate_code_verifier, QODER_CLIENT_TYPE, QODER_OPENAPI_COSY_VERSION,
};
use super::{
    LoginResult, LoginRuntime, LoginStep, ProviderError, ProviderPayload, RefreshedPayload,
};

pub const QODER_GLOBAL_GATEWAY: &str = "https://api3.qoder.sh/";
pub const QODER_CN_GATEWAY: &str = "https://gateway.qoder.com.cn/";
pub const QODER_GLOBAL_OPENAPI: &str = "https://openapi.qoder.sh";
pub const QODER_CN_OPENAPI: &str = "https://openapi.qoder.com.cn";
pub const QODER_GLOBAL_CENTER: &str = "https://center.qoder.sh";
pub const QODER_CN_CENTER: &str = "https://gateway.qoder.com.cn";
/// 浏览器授权页。Global 用 qoder.com；CN 用 qoder.cn（官方 Qoder CN 客户端
/// 实际使用的授权页，国内直连可达；从其客户端缓存抓包确认）。
pub const QODER_GLOBAL_DEVICE_LOGIN_URL: &str = "https://qoder.com/device/selectAccounts";
pub const QODER_CN_DEVICE_LOGIN_URL: &str = "https://qoder.cn/device/selectAccounts";
/// 官方 CLI 设备码授权页固定携带的公共 client id（生产客户端，从
/// qodercli 1.1.72 bundle 解包确认；缺失时授权页无法绑定设备授权，
/// 表现为浏览器登录成功但轮询一直 404）。
pub const QODER_DEVICE_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";
/// PAT 管理页（UI 展示用）。
pub const QODER_GLOBAL_PAT_PAGE: &str = "https://qoder.com/account/integrations";
pub const QODER_CN_PAT_PAGE: &str = "https://qoder.com.cn/account/integrations";

pub const QODER_LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const QODER_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL_SECS: u64 = 2;
/// 设备码轮询上限（与官方 CLI 的 5 分钟窗口对齐：2s × 150）。
const POLL_MAX_ATTEMPTS: u32 = 150;

const USER_AGENT: &str = "WaLiAPI";

/// Qoder 区域配置。两区都支持浏览器设备码登录（CN 链路经官方客户端抓包
/// 验证：授权页 qoder.cn/device/selectAccounts + openapi.qoder.com.cn
/// 轮询，官方客户端多带一个 client_id 参数，轮询端点对该参数不校验）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QoderRegion {
    Global,
    Cn,
}

impl QoderRegion {
    pub fn gateway(&self) -> &'static str {
        match self {
            Self::Global => QODER_GLOBAL_GATEWAY,
            Self::Cn => QODER_CN_GATEWAY,
        }
    }

    pub fn openapi(&self) -> &'static str {
        match self {
            Self::Global => QODER_GLOBAL_OPENAPI,
            Self::Cn => QODER_CN_OPENAPI,
        }
    }

    pub fn center(&self) -> &'static str {
        match self {
            Self::Global => QODER_GLOBAL_CENTER,
            Self::Cn => QODER_CN_CENTER,
        }
    }

    pub fn device_login_url(&self) -> &'static str {
        match self {
            Self::Global => QODER_GLOBAL_DEVICE_LOGIN_URL,
            Self::Cn => QODER_CN_DEVICE_LOGIN_URL,
        }
    }

    pub fn pat_page(&self) -> &'static str {
        match self {
            Self::Global => QODER_GLOBAL_PAT_PAGE,
            Self::Cn => QODER_CN_PAT_PAGE,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Global => "Qoder",
            Self::Cn => "Qoder CN",
        }
    }

    pub fn supports_browser_login(&self) -> bool {
        true
    }
}

/// 解析 `expires_at`（RFC3339/毫秒时间戳）或 `expires_in`（毫秒或秒）。
fn parse_expiry(expires_at: Option<&str>, expires_in_ms: Option<u64>) -> chrono::DateTime<Utc> {
    if let Some(text) = expires_at {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
            return parsed.with_timezone(&Utc);
        }
        if let Ok(ms) = text.parse::<i64>() {
            return Utc::now() + ChronoDuration::milliseconds(ms);
        }
    }
    if let Some(ms) = expires_in_ms {
        if ms > 0 {
            // 观测到的 expires_in 单位是毫秒（86_400_000 = 24h）。小数值
            // （< 10^7 ≈ 2.8h）按秒解释兼容两类服务端。
            if ms >= 10_000_000 {
                return Utc::now() + ChronoDuration::milliseconds(ms as i64);
            }
            return Utc::now() + ChronoDuration::seconds(ms as i64);
        }
    }
    // job token 默认 24h；拿不到显式过期时按保守值计算。
    Utc::now() + ChronoDuration::hours(24)
}

/// 登录成功后的凭据集合。
#[derive(Clone)]
pub struct QoderTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    pub machine_id: String,
    pub expires_at: chrono::DateTime<Utc>,
}

/// 设备码轮询响应（openapi `/api/v1/deviceToken/poll`）。
#[derive(serde::Deserialize)]
struct DevicePollWire {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl std::fmt::Debug for DevicePollWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePollWire")
            .field("token_present", &self.token.is_some())
            .field("refresh_token_present", &self.refresh_token.is_some())
            .field("user_id_present", &self.user_id.is_some())
            .finish()
    }
}

/// userinfo 响应（openapi `/api/v1/userinfo`）。
#[derive(serde::Deserialize, Default)]
struct UserInfoWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Clone)]
pub struct QoderLogin {
    client: reqwest::Client,
    region: QoderRegion,
    /// 测试注入：覆盖各端点前缀。
    gateway_base: Option<String>,
    openapi_base: Option<String>,
    center_base: Option<String>,
    device_login_url: Option<String>,
}

impl QoderLogin {
    pub fn new(region: QoderRegion) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(QODER_HTTP_TIMEOUT)
                .build()
                .expect("qoder login http client"),
            region,
            gateway_base: None,
            openapi_base: None,
            center_base: None,
            device_login_url: None,
        }
    }

    /// 测试构造器：允许指向本地 mock 端点，绝不触碰真实 Qoder 服务。
    pub fn with_endpoints(
        region: QoderRegion,
        gateway_base: impl Into<String>,
        openapi_base: impl Into<String>,
        center_base: impl Into<String>,
        device_login_url: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(QODER_HTTP_TIMEOUT)
                .build()
                .expect("qoder login http client"),
            region,
            gateway_base: Some(gateway_base.into()),
            openapi_base: Some(openapi_base.into()),
            center_base: Some(center_base.into()),
            device_login_url: Some(device_login_url.into()),
        }
    }

    /// 测试注入场景下按区域重建登录实例（端点覆盖保留给构造时指定者）。
    /// 生产实例端点为 None，重建后仍走固定 HTTPS 端点。
    pub fn clone_for_region_if_test(&self, region: QoderRegion) -> Self {
        if region == self.region && self.gateway_base.is_some() {
            return self.clone();
        }
        if let (Some(gateway), Some(openapi), Some(center), Some(device)) = (
            self.gateway_base.clone(),
            self.openapi_base.clone(),
            self.center_base.clone(),
            self.device_login_url.clone(),
        ) {
            return Self::with_endpoints(region, gateway, openapi, center, device);
        }
        Self::new(region)
    }

    fn openapi(&self) -> String {
        self.openapi_base
            .clone()
            .unwrap_or_else(|| self.region.openapi().to_owned())
    }

    fn center(&self) -> String {
        self.center_base
            .clone()
            .unwrap_or_else(|| self.region.center().to_owned())
    }

    fn refresh_url(&self) -> String {
        format!(
            "{}/algo/api/v3/user/refresh_token",
            self.center().trim_end_matches('/')
        )
    }

    fn exchange_url(&self) -> String {
        format!(
            "{}/api/v1/jobToken/exchange",
            self.openapi().trim_end_matches('/')
        )
    }

    fn userinfo_url(&self) -> String {
        format!("{}/api/v1/userinfo", self.openapi().trim_end_matches('/'))
    }

    fn device_poll_url(&self, nonce: &str, verifier: &str) -> String {
        format!(
            "{}/api/v1/deviceToken/poll?nonce={}&verifier={}&challenge_method=S256",
            self.openapi().trim_end_matches('/'),
            urlencoding(nonce),
            urlencoding(verifier),
        )
    }

    fn device_login_url(&self, challenge: &str, machine_id: &str, nonce: &str) -> String {
        let base = self
            .device_login_url
            .clone()
            .unwrap_or_else(|| self.region.device_login_url().to_owned());
        format!(
            "{base}?challenge={challenge}&challenge_method=S256&machine_id={machine}&nonce={nonce}&client_id={client_id}",
            challenge = urlencoding(challenge),
            machine = urlencoding(machine_id),
            nonce = urlencoding(nonce),
            client_id = QODER_DEVICE_CLIENT_ID,
        )
    }

    fn openapi_headers(&self, bearer: Option<&str>) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Accept".to_owned(), "application/json".to_owned()),
            ("User-Agent".to_owned(), USER_AGENT.to_owned()),
            (
                "Cosy-Version".to_owned(),
                QODER_OPENAPI_COSY_VERSION.to_owned(),
            ),
            ("Cosy-Clienttype".to_owned(), QODER_CLIENT_TYPE.to_owned()),
        ];
        if let Some(token) = bearer {
            headers.push((AUTHORIZATION.as_str().to_owned(), format!("Bearer {token}")));
        }
        headers
    }

    /// 交互式登录。`pat` 非空走 PAT 交换；否则走浏览器设备码流程（两区都支持，
    /// CN 授权页 qoder.cn 国内直连可达）。
    pub async fn login(
        &self,
        runtime: &dyn LoginRuntime,
        pat: Option<&str>,
        machine_id: &str,
    ) -> Result<LoginResult, ProviderError> {
        if let Some(pat) = pat.map(str::trim).filter(|p| !p.is_empty()) {
            return self.login_with_pat(runtime, pat, machine_id).await;
        }
        self.login_with_device_flow(runtime, machine_id).await
    }

    /// PAT 登录：交换 job token → 解析身份 → 组装结果。
    pub async fn login_with_pat(
        &self,
        runtime: &dyn LoginRuntime,
        pat: &str,
        machine_id: &str,
    ) -> Result<LoginResult, ProviderError> {
        if runtime.is_cancelled() {
            return Err(ProviderError::LoginCancelled);
        }
        runtime.set_step(LoginStep::Exchanging).await;
        let exchanged = self.exchange_pat(pat).await?;
        runtime.set_step(LoginStep::Syncing).await;
        let identity = self.fetch_userinfo(&exchanged.access_token).await;
        let tokens = QoderTokens {
            access_token: exchanged.access_token,
            refresh_token: exchanged.refresh_token,
            user_id: identity.0,
            email: identity.1,
            name: identity.2,
            machine_id: machine_id.to_owned(),
            expires_at: exchanged.expires_at,
        };
        Ok(self.login_result(tokens, pat.to_owned()))
    }

    /// 浏览器设备码登录（PKCE + nonce 轮询）。
    ///
    /// Qoder 的 poll 端点没有 RFC 8628 的 `expired_token` 语义，超时统一由
    /// 总 deadline 控制，因此不像 Kimi 那样需要外层重发循环。
    pub async fn login_with_device_flow(
        &self,
        runtime: &dyn LoginRuntime,
        machine_id: &str,
    ) -> Result<LoginResult, ProviderError> {
        let deadline = tokio::time::Instant::now() + QODER_LOGIN_TIMEOUT;
        if runtime.is_cancelled() {
            return Err(ProviderError::LoginCancelled);
        }
        runtime.set_step(LoginStep::Preparing).await;

        let verifier = generate_code_verifier();
        let challenge = code_challenge_s256(&verifier);
        let nonce = uuid::Uuid::new_v4().to_string();
        let verification_url = self.device_login_url(&challenge, machine_id, &nonce);

        // 先呈现授权 URL（浏览器打开失败仍可手动复制），再尝试打开浏览器。
        // Qoder 的授权页没有独立 user code，UI 展示 nonce 供比对。
        runtime
            .present_device_authorization(&verification_url, &nonce, None)
            .await?;
        match runtime.open_browser(&verification_url).await {
            Ok(()) => {}
            Err(error) => {
                if !runtime.is_cancelled() {
                    tracing::warn!("could not open Qoder authorization browser: {error}");
                }
            }
        }

        runtime.set_step(LoginStep::Waiting).await;
        let poll = match self
            .poll_device_token(runtime, &nonce, &verifier, deadline)
            .await
        {
            Ok(poll) => poll,
            Err(PollError::Timeout) => return Err(ProviderError::LoginTimeout),
            Err(PollError::Cancelled) => return Err(ProviderError::LoginCancelled),
            Err(PollError::ClientError) => return Err(ProviderError::TokenExchangeFailed),
        };
        runtime.set_step(LoginStep::Exchanging).await;
        let identity = self.fetch_userinfo(&poll.access_token).await;
        let tokens = QoderTokens {
            access_token: poll.access_token,
            refresh_token: poll.refresh_token,
            user_id: identity.0,
            email: identity.1,
            name: identity.2,
            machine_id: machine_id.to_owned(),
            expires_at: poll.expires_at,
        };
        Ok(self.login_result(tokens, String::new()))
    }

    async fn poll_device_token(
        &self,
        runtime: &dyn LoginRuntime,
        nonce: &str,
        verifier: &str,
        deadline: tokio::time::Instant,
    ) -> Result<ExchangedTokens, PollError> {
        let poll_url = self.device_poll_url(nonce, verifier);
        for _ in 0..POLL_MAX_ATTEMPTS {
            if runtime.is_cancelled() {
                return Err(PollError::Cancelled);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(PollError::Timeout);
            }
            let response = match self
                .client
                .get(&poll_url)
                .header(ACCEPT, "application/json")
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .send()
                .await
            {
                Ok(response) => response,
                Err(_) => {
                    poll_sleep(runtime).await;
                    continue;
                }
            };
            match response.status().as_u16() {
                // 202 / 404 = 尚未授权
                202 | 404 => poll_sleep(runtime).await,
                200 => {
                    let wire: DevicePollWire =
                        response.json().await.map_err(|_| PollError::ClientError)?;
                    let token = wire
                        .token
                        .filter(|t| !t.is_empty())
                        .ok_or(PollError::ClientError)?;
                    return Ok(ExchangedTokens {
                        access_token: token,
                        refresh_token: wire.refresh_token.unwrap_or_default(),
                        expires_at: parse_expiry(wire.expires_at.as_deref(), wire.expires_in),
                    });
                }
                status if (500..600).contains(&status) => poll_sleep(runtime).await,
                _ => return Err(PollError::ClientError),
            }
        }
        Err(PollError::Timeout)
    }

    /// PAT 交换 job token。该端点不需要 COSY 签名。
    async fn exchange_pat(&self, pat: &str) -> Result<ExchangedTokens, ProviderError> {
        let response = self
            .client
            .post(self.exchange_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header("Cosy-Version", QODER_OPENAPI_COSY_VERSION)
            .header("Cosy-Clienttype", QODER_CLIENT_TYPE)
            .json(&json!({ "personal_token": pat }))
            .send()
            .await
            .map_err(|_| ProviderError::Retryable)?;
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(ProviderError::Unauthorized);
        }
        if !status.is_success() {
            return Err(ProviderError::TokenExchangeFailed);
        }
        let wire: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
        let token = wire
            .get("token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or(ProviderError::Protocol)?;
        Ok(ExchangedTokens {
            access_token: token.to_owned(),
            refresh_token: wire
                .get("refresh_token")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            expires_at: parse_expiry(
                wire.get("expires_at").and_then(Value::as_str),
                wire.get("expires_in").and_then(Value::as_u64),
            ),
        })
    }

    /// 解析用户身份（尽力而为；失败时回退占位值，不阻塞登录）。
    /// 返回 `(user_id, email, name)`。
    async fn fetch_userinfo(&self, access_token: &str) -> (String, String, String) {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in self.openapi_headers(Some(access_token)) {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(&value),
            ) {
                headers.insert(name, value);
            }
        }
        let Ok(response) = self
            .client
            .get(self.userinfo_url())
            .headers(headers)
            .send()
            .await
        else {
            return fallback_identity(self.region);
        };
        if !response.status().is_success() {
            return fallback_identity(self.region);
        }
        let Ok(wire) = response.json::<UserInfoWire>().await else {
            return fallback_identity(self.region);
        };
        (
            wire.id.unwrap_or_default(),
            wire.email.unwrap_or_default(),
            wire.name.or(wire.username).unwrap_or_default(),
        )
    }

    /// 刷新 access token。payload 的 `refresh_token` 字段：
    /// - PAT 凭据存原始 PAT（`pt-...`），刷新即重新交换；
    /// - 设备码凭据存 `refresh_token|user_id|machine_id` 组合串。
    pub async fn refresh_payload(
        &self,
        payload: &ProviderPayload,
    ) -> Result<RefreshedPayload, ProviderError> {
        let value = payload.as_value();
        let refresh = value
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(ProviderError::InvalidPayload)?;
        let access = value
            .get("access_token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let machine_id = value
            .get("machine_id")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let (new_access, new_refresh, expires_at);
        if refresh.starts_with("pt-") {
            // PAT 凭据：重新交换。
            let exchanged = self.exchange_pat(refresh).await?;
            new_access = exchanged.access_token;
            new_refresh = exchanged.refresh_token;
            expires_at = exchanged.expires_at;
        } else {
            // 组合串：refresh_token|user_id|machine_id。
            let pure_refresh = refresh.split('|').next().unwrap_or(refresh).to_owned();
            let response = self
                .client
                .post(self.refresh_url())
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json")
                .header(AUTHORIZATION, format!("Bearer {access}"))
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .json(&json!({ "refreshToken": pure_refresh }))
                .send()
                .await
                .map_err(|_| ProviderError::Retryable)?;
            let status = response.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::Unauthorized);
            }
            if !status.is_success() {
                return Err(ProviderError::Retryable);
            }
            let wire: Value = response.json().await.map_err(|_| ProviderError::Protocol)?;
            new_access = wire
                .get("token")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .ok_or(ProviderError::Protocol)?
                .to_owned();
            new_refresh = wire
                .get("refresh_token")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .unwrap_or(pure_refresh);
            expires_at = parse_expiry(
                wire.get("expires_at").and_then(Value::as_str),
                wire.get("expires_in").and_then(Value::as_u64),
            );
        }

        // 刷新后尽力回填身份（不失败回滚——access token 本身有效即可路由）。
        let identity = self.fetch_userinfo(&new_access).await;
        let now = Utc::now();
        let tokens = QoderTokens {
            access_token: new_access,
            refresh_token: new_refresh,
            user_id: identity.0,
            email: identity.1,
            name: identity.2,
            machine_id: machine_id.to_owned(),
            expires_at,
        };
        Ok(RefreshedPayload {
            payload: self.payload(&tokens, self.pat_of(refresh)),
            last_refreshed_at: Some(now.to_rfc3339()),
            next_refresh_after: None,
            next_retry_after: None,
        })
    }

    /// PAT 凭据的 refresh_token 字段保存原始 PAT；设备码凭据保存组合串。
    fn pat_of(&self, refresh_token: &str) -> String {
        if refresh_token.starts_with("pt-") {
            refresh_token.to_owned()
        } else {
            String::new()
        }
    }

    fn payload(&self, tokens: &QoderTokens, pat: String) -> ProviderPayload {
        // PAT 凭据把原始 PAT 存为 refresh_token（刷新即重交换）；
        // 设备码凭据把组合串存为 refresh_token。
        let refresh_token = if !pat.is_empty() {
            pat
        } else {
            format!(
                "{}|{}|{}",
                tokens.refresh_token, tokens.user_id, tokens.machine_id
            )
        };
        ProviderPayload::new(json!({
            "version": 1,
            "access_token": tokens.access_token,
            "refresh_token": refresh_token,
            "user_id": tokens.user_id,
            "email": tokens.email,
            "name": tokens.name,
            "machine_id": tokens.machine_id,
            "region": match self.region {
                QoderRegion::Global => "global",
                QoderRegion::Cn => "cn",
            },
            "expires_at": tokens.expires_at.to_rfc3339(),
        }))
    }

    fn login_result(&self, tokens: QoderTokens, pat: String) -> LoginResult {
        let now = Utc::now();
        LoginResult {
            account_id: if tokens.user_id.is_empty() {
                tokens.machine_id.clone()
            } else {
                tokens.user_id.clone()
            },
            label: self.region.display_name().to_owned(),
            attributes: json!({
                "region": match self.region {
                    QoderRegion::Global => "global",
                    QoderRegion::Cn => "cn",
                },
                "plan_type": "qoder",
                "identity_source": if pat.is_empty() { "device_flow" } else { "pat" },
            }),
            payload: self.payload(&tokens, pat),
            last_refreshed_at: Some(now.to_rfc3339()),
            next_refresh_after: None,
            next_retry_after: None,
        }
    }
}

/// PAT 交换/设备码轮询共用的凭据产出。
struct ExchangedTokens {
    access_token: String,
    refresh_token: String,
    expires_at: chrono::DateTime<Utc>,
}

enum PollError {
    Timeout,
    Cancelled,
    ClientError,
}

fn fallback_identity(region: QoderRegion) -> (String, String, String) {
    match region {
        QoderRegion::Global => (
            "qoder-user".to_owned(),
            "user@qoder.com".to_owned(),
            "Qoder User".to_owned(),
        ),
        QoderRegion::Cn => (
            "qoder-user".to_owned(),
            "user@qoder.com.cn".to_owned(),
            "Qoder CN User".to_owned(),
        ),
    }
}

/// 最小 URL 查询编码（字母数字与 `-._~` 之外的字符按百分号转义）。
fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 等待一个轮询间隔；取消信号立即打断。
async fn poll_sleep(runtime: &dyn LoginRuntime) {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)) => {}
        _ = runtime.cancelled() => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use axum::{
        extract::State,
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};

    use super::super::{LoginRuntime, LoginStep};
    use super::*;

    const MACHINE_ID: &str = "m-machine-1";

    #[derive(Clone)]
    struct TestRuntime {
        cancel: Arc<tokio::sync::watch::Sender<bool>>,
        _rx: Arc<tokio::sync::watch::Receiver<bool>>,
    }
    impl Default for TestRuntime {
        fn default() -> Self {
            let (tx, rx) = tokio::sync::watch::channel(false);
            Self {
                cancel: Arc::new(tx),
                _rx: Arc::new(rx),
            }
        }
    }
    #[async_trait::async_trait]
    impl LoginRuntime for TestRuntime {
        async fn open_browser(&self, _url: &str) -> Result<(), ProviderError> {
            Ok(())
        }
        async fn set_step(&self, _step: LoginStep) {}
        async fn present_device_authorization(
            &self,
            _url: &str,
            _code: &str,
            _expires_at: Option<String>,
        ) -> Result<(), ProviderError> {
            Ok(())
        }
        fn is_cancelled(&self) -> bool {
            *self.cancel.borrow()
        }
        async fn cancelled(&self) {
            std::future::pending::<()>().await;
        }
    }

    #[derive(Clone, Default)]
    struct MockState {
        poll_hits: Arc<AtomicUsize>,
        poll_authorized: Arc<AtomicBool>,
        exchange_seen: Arc<Mutex<Vec<Value>>>,
        refresh_seen: Arc<Mutex<Vec<(String, Value)>>>,
        poll_response: Arc<Mutex<Value>>,
    }

    async fn mock(region: QoderRegion) -> (QoderLogin, MockState, String) {
        let state = MockState::default();
        let poll_state = state.clone();
        let exchange_state = state.clone();
        let refresh_state = state.clone();
        let app = Router::new()
            .route(
                "/api/v1/deviceToken/poll",
                get(
                    move |s: State<MockState>, uri: axum::extract::OriginalUri| {
                        let s = s.clone();
                        async move {
                            s.poll_hits.fetch_add(1, Ordering::SeqCst);
                            let _ = uri;
                            if !s.poll_authorized.load(Ordering::SeqCst) {
                                return (
                                    axum::http::StatusCode::NOT_FOUND,
                                    Json(json!({"error": "pending"})),
                                )
                                    .into_response();
                            }
                            (
                                axum::http::StatusCode::OK,
                                Json(s.poll_response.lock().unwrap().clone()),
                            )
                                .into_response()
                        }
                    },
                ),
            )
            .route(
                "/api/v1/jobToken/exchange",
                post(move |s: State<MockState>, body: Json<Value>| {
                    let s = s.clone();
                    async move {
                        s.exchange_seen.lock().unwrap().push(body.0);
                        Json(json!({
                            "token": "jt-test-token",
                            "refresh_token": "jrt-test-refresh",
                            "expires_in": 86_400_000i64
                        }))
                    }
                }),
            )
            .route(
                "/api/v1/userinfo",
                get(move || async {
                    Json(json!({"id": "u-123", "email": "dev@example.com", "name": "Dev"}))
                }),
            )
            .route(
                "/algo/api/v3/user/refresh_token",
                post(
                    move |s: State<MockState>,
                          headers: axum::http::HeaderMap,
                          body: Json<Value>| {
                        let s = s.clone();
                        async move {
                            let auth = headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_owned();
                            s.refresh_seen.lock().unwrap().push((auth, body.0));
                            Json(json!({
                                "token": "jt-refreshed",
                                "refresh_token": "jrt-rotated",
                                "expires_in": 86_400_000i64
                            }))
                        }
                    },
                ),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let login = QoderLogin::with_endpoints(
            region,
            format!("http://{addr}/"),
            format!("http://{addr}"),
            format!("http://{addr}"),
            format!("http://{addr}/device"),
        );
        (login, state, addr)
    }

    #[tokio::test]
    async fn pat_login_exchanges_and_resolves_identity() {
        let (login, state, _addr) = mock(QoderRegion::Cn).await;
        let result = login
            .login(&TestRuntime::default(), Some("pt-abc123"), MACHINE_ID)
            .await
            .unwrap();
        // 交换请求体是 {personal_token}
        let seen = state.exchange_seen.lock().unwrap();
        assert_eq!(seen[0]["personal_token"], "pt-abc123");
        // payload 携带身份与区域
        let payload = result.payload.as_value();
        assert_eq!(payload["access_token"], "jt-test-token");
        assert_eq!(payload["user_id"], "u-123");
        assert_eq!(payload["email"], "dev@example.com");
        assert_eq!(payload["name"], "Dev");
        assert_eq!(payload["region"], "cn");
        assert_eq!(payload["machine_id"], MACHINE_ID);
        // PAT 凭据的 refresh_token 就是原始 PAT（刷新即重交换）
        assert_eq!(payload["refresh_token"], "pt-abc123");
        // account_id 用 userinfo 的 id
        assert_eq!(result.account_id, "u-123");
        assert_eq!(result.label, "Qoder CN");
    }

    #[tokio::test]
    async fn cn_region_device_flow_also_supported() {
        // 官方 Qoder CN 客户端同样使用设备码登录（授权页 qoder.cn +
        // openapi.qoder.com.cn 轮询）；CN 不带 PAT 也应能走设备码流程。
        let (login, state, _addr) = mock(QoderRegion::Cn).await;
        *state.poll_response.lock().unwrap() = json!({
            "token": "jt-cn-device",
            "refresh_token": "jrt-cn-refresh",
            "expires_in": 86_400_000i64
        });
        state.poll_authorized.store(true, Ordering::SeqCst);
        let result = login.login(&TestRuntime::default(), None, MACHINE_ID).await;
        // 授权页 URL 由 runtime 呈现；这里只需要确认流程走通。
        assert!(
            result.is_ok(),
            "CN device flow should succeed: {:?}",
            result.err()
        );
        assert_eq!(
            result.unwrap().payload.as_value()["access_token"],
            "jt-cn-device"
        );
    }

    #[tokio::test]
    async fn device_flow_polls_until_authorized() {
        let (login, state, _addr) = mock(QoderRegion::Global).await;
        *state.poll_response.lock().unwrap() = json!({
            "token": "jt-device-token",
            "refresh_token": "jrt-device-refresh",
            "user_id": "u-999",
            "expires_in": 86_400_000i64
        });
        let runtime = TestRuntime::default();
        let handle = {
            let login = login.clone();
            let runtime = runtime.clone();
            tokio::spawn(async move { login.login(&runtime, None, MACHINE_ID).await })
        };
        // 给几轮 pending 轮询的时间，然后放行授权。
        tokio::time::sleep(Duration::from_millis(2500)).await;
        state.poll_authorized.store(true, Ordering::SeqCst);
        let result = handle.await.unwrap().unwrap();
        assert!(state.poll_hits.load(Ordering::SeqCst) >= 1);
        let payload = result.payload.as_value();
        assert_eq!(payload["access_token"], "jt-device-token");
        // refresh 后 refresh_token 重组为组合串：jrt-device-refresh|user_id|machine_id。
        // 注意 user_id 来自登录后的 userinfo 回填（mock 返回 u-123），
        // 不是 poll 响应里的 u-999。
        let refresh = payload["refresh_token"].as_str().unwrap();
        assert!(
            refresh.starts_with("jrt-device-refresh|u-123|m-machine-1"),
            "refresh should be combined string, got {refresh}"
        );
    }

    #[tokio::test]
    async fn refresh_pat_reexchanges() {
        let (login, state, _addr) = mock(QoderRegion::Cn).await;
        let payload = ProviderPayload::new(json!({
            "version": 1,
            "access_token": "jt-old",
            "refresh_token": "pt-keep",
            "user_id": "u-123",
            "email": "dev@example.com",
            "name": "Dev",
            "machine_id": MACHINE_ID,
            "region": "cn",
            "expires_at": "2020-01-01T00:00:00Z"
        }));
        let refreshed = login.refresh_payload(&payload).await.unwrap();
        assert_eq!(
            refreshed.payload.as_value()["access_token"],
            "jt-test-token"
        );
        assert_eq!(refreshed.payload.as_value()["refresh_token"], "pt-keep");
        let seen = state.exchange_seen.lock().unwrap();
        assert_eq!(seen[0]["personal_token"], "pt-keep");
    }

    #[tokio::test]
    async fn refresh_device_flow_uses_combined_refresh_token() {
        let (login, state, _addr) = mock(QoderRegion::Global).await;
        let payload = ProviderPayload::new(json!({
            "version": 1,
            "access_token": "jt-old",
            "refresh_token": format!("jrt-old|u-1|{MACHINE_ID}"),
            "user_id": "u-1",
            "machine_id": MACHINE_ID,
            "region": "global",
            "expires_at": "2020-01-01T00:00:00Z"
        }));
        let refreshed = login.refresh_payload(&payload).await.unwrap();
        assert_eq!(refreshed.payload.as_value()["access_token"], "jt-refreshed");
        let seen = state.refresh_seen.lock().unwrap();
        // Bearer 携带旧 access token；body 只带纯 refresh token
        assert_eq!(seen[0].0, "Bearer jt-old");
        assert_eq!(seen[0].1["refreshToken"], "jrt-old");
        // 刷新后 refresh_token 重组为组合串（新的 refresh）
        let new_refresh = refreshed.payload.as_value()["refresh_token"]
            .as_str()
            .unwrap();
        assert!(
            new_refresh.starts_with("jrt-rotated|u-123|"),
            "got {new_refresh}"
        );
    }

    #[test]
    fn expiry_parsing_handles_rfc3339_and_millis() {
        let now = Utc::now();
        let from_rfc = parse_expiry(Some("2030-01-01T00:00:00Z"), None);
        assert!(from_rfc > now + ChronoDuration::days(100));
        // 毫秒级 expires_in（参考实现观测值）
        let from_ms = parse_expiry(None, Some(86_400_000));
        assert!(from_ms > now + ChronoDuration::hours(23));
        assert!(from_ms < now + ChronoDuration::hours(25));
        // 秒级 expires_in（<= 10 天都按秒解释）
        let from_secs = parse_expiry(None, Some(3600));
        assert!(
            (from_secs - (now + ChronoDuration::hours(1)))
                .num_seconds()
                .abs()
                < 60
        );
        // 完全缺失 → 24h 默认
        let fallback = parse_expiry(None, None);
        assert!(
            (fallback - (now + ChronoDuration::hours(24)))
                .num_seconds()
                .abs()
                < 60
        );
    }

    #[test]
    fn urlencoding_matches_rfc3986_unreserved() {
        assert_eq!(urlencoding("abc-._~123"), "abc-._~123");
        assert_eq!(urlencoding("a b"), "a%20b");
        assert_eq!(urlencoding("a+b"), "a%2Bb");
    }

    #[test]
    fn constants_are_fixed() {
        assert_eq!(QoderRegion::Global.gateway(), "https://api3.qoder.sh/");
        assert_eq!(QoderRegion::Cn.openapi(), "https://openapi.qoder.com.cn");
        // 两区都支持浏览器设备码登录（CN 授权页 qoder.cn 经官方客户端抓包验证）。
        assert!(QoderRegion::Global.supports_browser_login());
        assert!(QoderRegion::Cn.supports_browser_login());
        assert_eq!(
            QoderRegion::Cn.device_login_url(),
            "https://qoder.cn/device/selectAccounts"
        );
        assert_eq!(
            QoderRegion::Global.device_login_url(),
            "https://qoder.com/device/selectAccounts"
        );
    }
}
