//! Qoder COSY 请求签名（逆向自 Qoder CLI 私有网关协议）。
//!
//! 每个发往 Qoder 网关的请求必须携带 `Authorization: Bearer COSY.<payload>.<sig>`
//! 以及一组 `Cosy-*` 头。签名流程：
//!
//! 1. 生成随机 16 字节 AES-128-CBC 密钥（IV 与密钥相同，与官方客户端一致）；
//! 2. 用该密钥加密用户身份 JSON（`{uid, security_oauth_token, name, aid, email}`）；
//! 3. 用内置 RSA 公钥加密 AES 密钥得到 `cosyKey`；
//! 4. `sig = MD5(payloadB64 \n cosyKey \n timestamp \n body \n sigPath)`；
//! 5. `payloadB64` 是 `{version, requestId, info(加密身份), cosyVersion, ideVersion}`
//!    的 Base64。
//!
//! 另含 Qoder 专有的「自定义字母表 Base64 + 分块重排」请求体编码
//! （[`qoder_encode_body`]）。算法均来自 MIT 协议参考实现
//! `pi-provider-qoder`（simonsmh），与本模块等价改写为 Rust。
//!
//! 本模块只做纯计算（签名/编码），不发任何 HTTP 请求。

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::Engine as _;
use md5::{Digest, Md5};
use rsa::{pkcs8::DecodePublicKey, Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::json;
use sha2::Sha256;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
/// 内置 Qoder 客户端 RSA 公钥（PEM，PKCS#1 SubjectPublicKeyInfo）。
const QODER_RSA_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDA8iMH5c02LilrsERw9t6Pv5Nc
4k6Pz1EaDicBMpdpxKduSZu5OANqUq8er4GM95omAGIOPOh+Nx0spthYA2BqGz+l
6HRkPJ7S236FZz73In/KVuLnwI8JJ2CbuJap8kvheCCZpmAWpb/cPx/3Vr/J6I17
XcW+ML9FoCI6AOvOzwIDAQAB
-----END PUBLIC KEY-----";

/// 与当前 Qoder CLI 目录协议对齐的 COSY 版本。过旧的值会让模型目录
/// 返回缩减列表。
pub const QODER_GATEWAY_COSY_VERSION: &str = "1.1.38";
/// openapi 域（PAT 交换 / userinfo）使用的 COSY 版本。
pub const QODER_OPENAPI_COSY_VERSION: &str = "1.0.1";
pub const QODER_CLIENT_TYPE: &str = "5";
const QODER_DATA_POLICY: &str = "disagree";
const QODER_LOGIN_VERSION: &str = "v2";

/// 按 `process.platform`/`arch` 派生的机器 OS 标识，与官方客户端取值一致。
pub fn qoder_machine_os() -> &'static str {
    if cfg!(target_os = "windows") {
        if cfg!(target_arch = "aarch64") {
            "aarch64_windows"
        } else {
            "x86_64_windows"
        }
    } else if cfg!(target_arch = "aarch64") {
        "aarch64_linux"
    } else {
        "x86_64_linux"
    }
}

/// 网关侧固定的机器类型 magic 值。
pub const QODER_MACHINE_TYPE: &str = "5";

/// COSY 签名所需的用户身份（来自登录 payload，非秘密字段除外）。
#[derive(Clone)]
pub struct CosyIdentity {
    pub user_id: String,
    pub auth_token: String,
    pub name: String,
    pub email: String,
    pub machine_id: String,
}

/// 解析内置公钥。公钥是编译期常量，解析失败属编程错误。
fn public_key() -> RsaPublicKey {
    RsaPublicKey::from_public_key_pem(QODER_RSA_PUBLIC_KEY)
        .expect("bundled Qoder RSA public key must parse")
}

/// AES-128-CBC（PKCS7）加密后 Base64。密钥与 IV 同值，对齐官方实现。
fn aes_encrypt_cbc_base64(plaintext: &str, key: &[u8; 16]) -> String {
    let encryptor = Aes128CbcEnc::new(key.into(), key.into());
    let ciphertext = encryptor.encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(ciphertext)
}

/// RSA PKCS#1 v1.5 加密后 Base64。
fn rsa_encrypt_base64(data: &[u8]) -> Result<String, rsa::Error> {
    let key = public_key();
    let mut rng = rsa::rand_core::OsRng;
    let encrypted = key.encrypt(&mut rng, Pkcs1v15Encrypt, data)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
}

/// URL 的签名路径：pathname，且剥掉开头的 `/algo` 前缀。
pub fn compute_sig_path(url: &str) -> String {
    let path = url::Url::parse(url)
        .map(|parsed| parsed.path().to_owned())
        .unwrap_or_default();
    path.strip_prefix("/algo").unwrap_or(&path).to_owned()
}

/// 分组 MD5：按官方模板字节序喂入，不拼接完整 body 字符串。
fn md5_hex_chunks(
    payload_b64: &str,
    cosy_key: &str,
    timestamp: &str,
    body: &[u8],
    sig_path: &str,
) -> String {
    let mut hasher = Md5::new();
    hasher.update(payload_b64.as_bytes());
    hasher.update(b"\n");
    hasher.update(cosy_key.as_bytes());
    hasher.update(b"\n");
    hasher.update(timestamp.as_bytes());
    hasher.update(b"\n");
    hasher.update(body);
    hasher.update(b"\n");
    hasher.update(sig_path.as_bytes());
    hex::encode(hasher.finalize())
}

fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// 一次请求的完整 COSY 鉴权头集合。
///
/// 返回值按 header 名排列；调用方应原样附加到出站请求，禁止下游覆盖。
pub fn build_cosy_headers(
    body: Option<&[u8]>,
    request_url: &str,
    identity: &CosyIdentity,
) -> Result<Vec<(String, String)>, rsa::Error> {
    // 官方实现用 UUID 简单形式（32 个十六进制字符）字符串的前 16 字节作为
    // AES-128 密钥字符串（IV 同值）。
    let aes_key_str: String = {
        let simple = uuid::Uuid::new_v4().simple().to_string();
        simple.bytes().take(16).map(|b| b as char).collect()
    };
    let mut key = [0u8; 16];
    for (slot, byte) in key.iter_mut().zip(aes_key_str.as_bytes()) {
        *slot = *byte;
    }

    let user_info = json!({
        "uid": identity.user_id,
        "security_oauth_token": identity.auth_token,
        "name": identity.name,
        "aid": "",
        "email": identity.email,
    });
    let info_b64 = aes_encrypt_cbc_base64(user_info.to_string().as_str(), &key);
    let cosy_key = rsa_encrypt_base64(key.as_slice())?;

    let timestamp = chrono::Utc::now().timestamp().to_string();
    let request_id = uuid::Uuid::new_v4().to_string();

    let cosy_payload = json!({
        "version": "v1",
        "requestId": request_id,
        "info": info_b64,
        "cosyVersion": QODER_GATEWAY_COSY_VERSION,
        "ideVersion": "",
    });
    let payload_b64 = base64::engine::general_purpose::STANDARD.encode(cosy_payload.to_string());
    let sig_path = compute_sig_path(request_url);

    let body_bytes: Vec<u8> = body.unwrap_or(&[]).to_vec();
    let sig = md5_hex_chunks(&payload_b64, &cosy_key, &timestamp, &body_bytes, &sig_path);
    let body_hash = md5_hex(&body_bytes);
    let body_len = body_bytes.len().to_string();

    Ok(vec![
        (
            "Authorization".to_owned(),
            format!("Bearer COSY.{payload_b64}.{sig}"),
        ),
        ("Cosy-Key".to_owned(), cosy_key),
        ("Cosy-User".to_owned(), identity.user_id.clone()),
        ("Cosy-Date".to_owned(), timestamp),
        (
            "Cosy-Version".to_owned(),
            QODER_GATEWAY_COSY_VERSION.to_owned(),
        ),
        ("Cosy-Machineid".to_owned(), identity.machine_id.clone()),
        ("Cosy-Machinetoken".to_owned(), identity.machine_id.clone()),
        ("Cosy-Machinetype".to_owned(), QODER_MACHINE_TYPE.to_owned()),
        ("Cosy-Machineos".to_owned(), qoder_machine_os().to_owned()),
        ("Cosy-Clienttype".to_owned(), QODER_CLIENT_TYPE.to_owned()),
        ("Cosy-Clientip".to_owned(), "127.0.0.1".to_owned()),
        ("Cosy-Bodyhash".to_owned(), body_hash),
        ("Cosy-Bodylength".to_owned(), body_len),
        ("Cosy-Sigpath".to_owned(), sig_path),
        ("Cosy-Data-Policy".to_owned(), QODER_DATA_POLICY.to_owned()),
        ("Cosy-Organization-Id".to_owned(), String::new()),
        ("Cosy-Organization-Tags".to_owned(), String::new()),
        ("Login-Version".to_owned(), QODER_LOGIN_VERSION.to_owned()),
        ("X-Request-Id".to_owned(), uuid::Uuid::new_v4().to_string()),
    ])
}

/// Qoder 专有 Base64 变体的自定义字母表（64 字符；`$` 作 padding 单独处理）。
const QODER_CUSTOM_ALPHABET: &[u8; 64] =
    b"_doRTgHZBKcGVjlvpC,@aFSx#DPuNJme&i*MzLOEn)sUrthbf%Y^w.(kIQyXqWA!";
const QODER_STD_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 用 Qoder 自定义字母表编码标准 Base64 输出。
fn qoder_alphabet_encode(std_b64: &str) -> Vec<u8> {
    std_b64
        .bytes()
        .map(|byte| {
            if byte == b'=' {
                b'$'
            } else {
                match QODER_STD_ALPHABET.iter().position(|c| *c == byte) {
                    Some(index) => QODER_CUSTOM_ALPHABET[index],
                    None => byte,
                }
            }
        })
        .collect()
}

/// Qoder 网关请求体编码：标准 Base64 → 自定义字母表 → 分块重排。
///
/// 重排规则（对齐官方客户端）：把标准 Base64 串按 `a = len/3` 分成三段，
/// 输出顺序为 `[尾段, 中段, 首段]`：
/// `std[n-a..n] + std[a..n-a] + std[0..a]`。
pub fn qoder_encode_body(plaintext: &[u8]) -> Vec<u8> {
    let std_b64 = base64::engine::general_purpose::STANDARD.encode(plaintext);
    let encoded = qoder_alphabet_encode(&std_b64);
    let n = encoded.len();
    let a = n / 3;
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(&encoded[n - a..n]);
    out.extend_from_slice(&encoded[a..n - a]);
    out.extend_from_slice(&encoded[0..a]);
    out
}

/// Qoder 网关可选的 PKCE code verifier（设备登录用）。
pub fn generate_code_verifier() -> String {
    use rsa::rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rsa::rand_core::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// S256 PKCE challenge：`base64url(sha256(verifier))`。
pub fn code_challenge_s256(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sig_path_strips_algo_prefix() {
        assert_eq!(
            compute_sig_path("https://api3.qoder.sh/algo/api/v2/x"),
            "/api/v2/x"
        );
        assert_eq!(
            compute_sig_path("https://api3.qoder.sh/api/v2/x"),
            "/api/v2/x"
        );
    }

    #[test]
    fn cosy_headers_shape() {
        let identity = CosyIdentity {
            user_id: "u1".into(),
            auth_token: "tok".into(),
            name: "tester".into(),
            email: "t@e.st".into(),
            machine_id: "m1".into(),
        };
        let headers = build_cosy_headers(
            Some(b"{}".as_slice()),
            "https://api3.qoder.sh/algo/api/v2/service/pro/sse/x",
            &identity,
        )
        .unwrap();
        let auth = &headers[0].1;
        assert!(auth.starts_with("Bearer COSY."));
        let get = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        assert_eq!(get("Cosy-User"), "u1");
        assert_eq!(get("Cosy-Machineid"), "m1");
        assert_eq!(get("Cosy-Version"), QODER_GATEWAY_COSY_VERSION);
        assert_eq!(get("Cosy-Clienttype"), "5");
        // sigpath 去掉 /algo 前缀
        assert_eq!(get("Cosy-Sigpath"), "/api/v2/service/pro/sse/x");
        // body hash 与 length 对应空 JSON 对象
        let empty_md5 = md5_hex(b"{}");
        assert_eq!(get("Cosy-Bodyhash"), empty_md5);
        assert_eq!(get("Cosy-Bodylength"), "2");
    }

    #[test]
    fn body_encoding_roundtrip_shape() {
        // 输出长度与标准 Base64 一致；padding 用 `$`；字符全部来自自定义字母表。
        // 注意：编码后还有分块重排，padding 不一定在末尾，这里只断言字符集与长度。
        let encoded = qoder_encode_body(b"hello world");
        let std = base64::engine::general_purpose::STANDARD.encode(b"hello world");
        assert_eq!(encoded.len(), std.len());
        assert!(std.ends_with('='));
        assert!(encoded.contains(&b'$'), "padding must be encoded as $");
        for byte in &encoded {
            assert!(
                QODER_CUSTOM_ALPHABET.contains(byte) || *byte == b'$',
                "byte {byte} not in custom alphabet"
            );
        }
    }

    #[test]
    fn body_encoding_block_reorder_is_documented_shape() {
        // n=16, a=5：输出 = std[11..16] + std[5..11] + std[0..5]
        let encoded = qoder_encode_body(b"1234567890ABCDEF");
        let std = qoder_alphabet_encode(
            base64::engine::general_purpose::STANDARD
                .encode(b"1234567890ABCDEF")
                .as_str(),
        );
        let n = std.len();
        let a = n / 3;
        let expected: Vec<u8> = std[n - a..]
            .iter()
            .chain(std[a..n - a].iter())
            .chain(std[0..a].iter())
            .copied()
            .collect();
        assert_eq!(encoded, expected);
    }

    #[test]
    fn pkce_round_trip() {
        let verifier = generate_code_verifier();
        let challenge = code_challenge_s256(&verifier);
        assert!(!verifier.is_empty());
        // challenge 是 verifier 的 sha256 base64url，可复算验证。
        assert_eq!(challenge, code_challenge_s256(&verifier));
        assert!(!challenge.contains('='));
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
    }
}
