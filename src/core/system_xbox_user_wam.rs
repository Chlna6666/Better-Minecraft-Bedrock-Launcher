#![cfg(target_os = "windows")]

use base64::Engine as _;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::rand_core::OsRng;
use secrecy::{ExposeSecret as _, SecretString};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::Security::Authentication::Web::Core::{
    WebAuthenticationCoreManager, WebTokenRequest, WebTokenRequestStatus,
};
use windows::core::HSTRING;

const MSA_CLIENT_ID: &str = "00000000402b5328";
const MSA_SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";
const DEVICE_AUTH_ENDPOINT: &str = "https://device.auth.xboxlive.com/device/authenticate";
const USER_AUTH_ENDPOINT: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_ENDPOINT: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const PROFILE_ENDPOINT: &str = "https://profile.xboxlive.com/users/batch/profile/settings";
const PROFILE_RELYING_PARTY: &str = "http://xboxlive.com";
const WINDOWS_FILE_TIME_EPOCH_OFFSET_SECONDS: u64 = 11_644_473_600;
const MAX_AVATAR_BYTES: usize = 8 * 1024 * 1024;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

struct DeviceIdentity {
    id: String,
    signing_key: SigningKey,
}

struct XboxToken {
    value: SecretString,
}

struct XboxTokenWithClaims {
    token: XboxToken,
    claims: XboxClaims,
}

#[derive(Clone, Debug, Default)]
struct XboxClaims {
    xuid: Option<String>,
    user_hash: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims", default)]
    display_claims: DisplayClaims,
}

#[derive(Default, Deserialize)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<HashMap<String, Value>>,
}

#[derive(Deserialize)]
struct ProfileResponse {
    #[serde(rename = "profileUsers", default)]
    profile_users: Vec<ProfileUser>,
}

#[derive(Deserialize)]
struct ProfileUser {
    id: String,
    #[serde(default)]
    settings: Vec<ProfileSetting>,
}

#[derive(Deserialize)]
struct ProfileSetting {
    id: String,
    value: String,
}

#[derive(Serialize)]
struct ProfileRequest<'a> {
    #[serde(rename = "userIds")]
    user_ids: [&'a str; 1],
    settings: [&'static str; 1],
}

pub(super) fn gamer_picture_for_xuid(expected_xuid: u64) -> Result<Option<Vec<u8>>, String> {
    if expected_xuid == 0 {
        return Err("本地 Xbox XUID 为空".to_string());
    }

    let msa_access_token = acquire_wam_token()?;
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent("BMCBL local Xbox avatar/1.0")
        .build()
        .map_err(|error| format!("创建 Xbox 本地头像 HTTP 客户端失败：{error}"))?;
    let identity = DeviceIdentity {
        id: format!("{{{}}}", uuid::Uuid::new_v4()),
        signing_key: SigningKey::random(&mut OsRng),
    };
    let proof_key = proof_key(&identity.signing_key)?;
    authenticate_device(&client, &identity, &proof_key)?;
    let user = authenticate_user(&client, &identity.signing_key, &msa_access_token)?;
    let xsts = authorize_xsts(&client, &identity.signing_key, &user)?;

    let actual_xuid = xsts
        .claims
        .xuid
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "XSTS 返回缺少有效 xid".to_string())?;
    if actual_xuid != expected_xuid {
        return Err(format!(
            "XSTS xid 与 Windows 本地账号不匹配：expected_xuid={expected_xuid}, xid={actual_xuid}"
        ));
    }

    let avatar_url = fetch_avatar_url(&client, &identity.signing_key, expected_xuid, &xsts)?;
    let Some(avatar_url) = avatar_url else {
        return Ok(None);
    };
    let response = client
        .get(avatar_url)
        .send()
        .map_err(|error| format!("下载 Xbox GameDisplayPicRaw 失败：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("下载 Xbox GameDisplayPicRaw 返回 HTTP {status}"));
    }
    let bytes = response
        .bytes()
        .map_err(|error| format!("读取 Xbox GameDisplayPicRaw 失败：{error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
        return Err("Xbox GameDisplayPicRaw 为空或超过大小限制".to_string());
    }
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err("Xbox GameDisplayPicRaw 不是 PNG 图片".to_string());
    }
    Ok(Some(bytes.to_vec()))
}

fn acquire_wam_token() -> Result<SecretString, String> {
    let scope = HSTRING::from(MSA_SCOPE);
    let client_id = HSTRING::from(MSA_CLIENT_ID);
    let mut failures = Vec::new();

    for provider_id in ["https://login.live.com", "https://login.microsoft.com"] {
        let operation = match WebAuthenticationCoreManager::FindAccountProviderAsync(
            &HSTRING::from(provider_id),
        ) {
            Ok(operation) => operation,
            Err(error) => {
                failures.push(format!(
                    "{provider_id}: WAM 查找账号提供程序失败：{error:?}"
                ));
                continue;
            }
        };
        let provider = match operation.join() {
            Ok(provider) => provider,
            Err(error) => {
                failures.push(format!("{provider_id}: {error}"));
                continue;
            }
        };
        let request = WebTokenRequest::Create(&provider, &scope, &client_id)
            .map_err(|error| format!("创建 WAM Xbox 请求失败：{error:?}"))?;

        let operation = match WebAuthenticationCoreManager::GetTokenSilentlyAsync(&request) {
            Ok(operation) => operation,
            Err(error) => {
                failures.push(format!("{provider_id}: 创建 WAM 静默请求失败：{error:?}"));
                continue;
            }
        };
        match operation.join() {
            Ok(result) => match token_from_wam_result(&result) {
                Ok(token) => return Ok(token),
                Err(error) => failures.push(format!("{provider_id}: {error}")),
            },
            Err(error) => failures.push(format!("{provider_id}: {error}")),
        }
    }

    Err(format!(
        "Windows WAM 未能静默取得本地 Xbox/Microsoft Access Token；{}",
        failures.join(" | ")
    ))
}

fn token_from_wam_result(
    result: &windows::Security::Authentication::Web::Core::WebTokenRequestResult,
) -> Result<SecretString, String> {
    let status = result
        .ResponseStatus()
        .map_err(|error| format!("读取 WAM 响应状态失败：{error:?}"))?;
    if status != WebTokenRequestStatus::Success {
        return Err(format!("WAM 响应状态为 {:?}", status.0));
    }
    let responses = result
        .ResponseData()
        .map_err(|error| format!("读取 WAM 响应令牌失败：{error:?}"))?;
    for index in 0..responses
        .Size()
        .map_err(|error| format!("读取 WAM 令牌数量失败：{error:?}"))?
    {
        let response = responses
            .GetAt(index)
            .map_err(|error| format!("读取 WAM 令牌项失败：{error:?}"))?;
        let token = response
            .Token()
            .map_err(|error| format!("读取 WAM 令牌文本失败：{error:?}"))?
            .to_string();
        if !token.trim().is_empty() {
            return Ok(SecretString::from(token));
        }
    }
    Err("WAM 响应没有可用令牌".to_string())
}

fn authenticate_device(
    client: &reqwest::blocking::Client,
    identity: &DeviceIdentity,
    proof_key: &Value,
) -> Result<XboxToken, String> {
    let response: TokenResponse = post_signed_json(
        client,
        &identity.signing_key,
        DEVICE_AUTH_ENDPOINT,
        "",
        &json!({
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
            "Properties": {
                "AuthMethod": "ProofOfPossession",
                "Id": identity.id,
                "DeviceType": "Win32",
                "Version": "10.0.22631",
                "ProofKey": proof_key,
            }
        }),
        "device-auth",
    )?;
    token_from_response(response)
}

fn authenticate_user(
    client: &reqwest::blocking::Client,
    signing_key: &SigningKey,
    msa_access_token: &SecretString,
) -> Result<XboxToken, String> {
    let token = msa_access_token.expose_secret().trim();
    let rps_ticket = if token.starts_with("t=") {
        token.to_string()
    } else {
        format!("t={token}")
    };
    let response: TokenResponse = post_signed_json(
        client,
        signing_key,
        USER_AUTH_ENDPOINT,
        "",
        &json!({
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": rps_ticket,
            }
        }),
        "user-auth",
    )?;
    token_from_response(response)
}

fn authorize_xsts(
    client: &reqwest::blocking::Client,
    signing_key: &SigningKey,
    user: &XboxToken,
) -> Result<XboxTokenWithClaims, String> {
    let response: TokenResponse = post_signed_json(
        client,
        signing_key,
        XSTS_ENDPOINT,
        "",
        &json!({
            "RelyingParty": PROFILE_RELYING_PARTY,
            "TokenType": "JWT",
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [user.value.expose_secret()],
            }
        }),
        "xsts-profile",
    )?;
    token_with_claims(response)
}

fn fetch_avatar_url(
    client: &reqwest::blocking::Client,
    signing_key: &SigningKey,
    expected_xuid: u64,
    token: &XboxTokenWithClaims,
) -> Result<Option<String>, String> {
    let user_hash = token
        .claims
        .user_hash
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "XSTS 返回缺少 user hash".to_string())?;
    let xuid = expected_xuid.to_string();
    let authorization = format!("XBL3.0 x={user_hash};{}", token.token.value.expose_secret());
    let response: ProfileResponse = post_signed_json(
        client,
        signing_key,
        PROFILE_ENDPOINT,
        &authorization,
        &ProfileRequest {
            user_ids: [&xuid],
            settings: ["GameDisplayPicRaw"],
        },
        "xbox-profile",
    )?;
    let Some(user) = response
        .profile_users
        .into_iter()
        .find(|user| user.id == xuid)
    else {
        return Err("Xbox Profile API 未返回当前本地用户".to_string());
    };
    Ok(user
        .settings
        .into_iter()
        .find(|setting| setting.id == "GameDisplayPicRaw")
        .map(|setting| setting.value)
        .filter(|value| value.starts_with("https://"))
        .map(|value| {
            if value.contains("&format=") {
                value
            } else {
                format!("{value}&format=png&w=208&h=208")
            }
        }))
}

fn proof_key(signing_key: &SigningKey) -> Result<Value, String> {
    let encoded = signing_key.verifying_key().to_encoded_point(false);
    let x = encoded.x().ok_or("P-256 公钥缺少 X 坐标")?;
    let y = encoded.y().ok_or("P-256 公钥缺少 Y 坐标")?;
    let engine = base64::engine::general_purpose::STANDARD;
    Ok(json!({
        "alg": "ES256",
        "crv": "P-256",
        "kty": "EC",
        "use": "sig",
        "x": engine.encode(x),
        "y": engine.encode(y),
    }))
}

fn post_signed_json<T, B>(
    client: &reqwest::blocking::Client,
    signing_key: &SigningKey,
    endpoint: &str,
    authorization: &str,
    body: &B,
    stage: &str,
) -> Result<T, String>
where
    T: DeserializeOwned,
    B: Serialize + ?Sized,
{
    let body =
        serde_json::to_vec(body).map_err(|error| format!("{stage}: JSON 编码失败：{error}"))?;
    let url =
        reqwest::Url::parse(endpoint).map_err(|error| format!("{stage}: URL 无效：{error}"))?;
    let path_and_query = url.query().map_or_else(
        || url.path().to_string(),
        |query| format!("{}?{query}", url.path()),
    );
    let signature = signature_header(signing_key, "POST", &path_and_query, authorization, &body)?;
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            "x-xbl-contract-version",
            if stage == "xbox-profile" { "2" } else { "1" },
        )
        .header("Signature", signature)
        .body(body);
    if !authorization.is_empty() {
        request = request.header(reqwest::header::AUTHORIZATION, authorization);
    }
    let response = request
        .send()
        .map_err(|error| format!("{stage}: Xbox 请求失败：{error}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|error| format!("{stage}: 读取 Xbox 响应失败：{error}"))?;
    if !status.is_success() {
        return Err(format!("{stage}: Xbox 服务返回 HTTP {status}"));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("{stage}: Xbox 返回无效 JSON：{error}"))
}

fn signature_header(
    signing_key: &SigningKey,
    method: &str,
    path_and_query: &str,
    authorization: &str,
    body: &[u8],
) -> Result<String, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间早于 Unix epoch".to_string())?
        .as_secs()
        .saturating_add(WINDOWS_FILE_TIME_EPOCH_OFFSET_SECONDS)
        .saturating_mul(10_000_000);
    let version = 1_u32.to_be_bytes();
    let timestamp = timestamp.to_be_bytes();
    let mut signed = Vec::with_capacity(
        version.len()
            + timestamp.len()
            + method.len()
            + path_and_query.len()
            + authorization.len()
            + body.len()
            + 6,
    );
    for field in [
        version.as_slice(),
        timestamp.as_slice(),
        method.as_bytes(),
        path_and_query.as_bytes(),
        authorization.as_bytes(),
        body,
    ] {
        signed.extend_from_slice(field);
        signed.push(0);
    }
    let signature: Signature = signing_key.sign(&signed);
    let mut header = Vec::with_capacity(76);
    header.extend_from_slice(&version);
    header.extend_from_slice(&timestamp);
    header.extend_from_slice(signature.to_bytes().as_ref());
    Ok(base64::engine::general_purpose::STANDARD.encode(header))
}

fn token_from_response(response: TokenResponse) -> Result<XboxToken, String> {
    if response.token.trim().is_empty() {
        return Err("Xbox 服务响应缺少 token".to_string());
    }
    Ok(XboxToken {
        value: SecretString::from(response.token),
    })
}

fn token_with_claims(response: TokenResponse) -> Result<XboxTokenWithClaims, String> {
    let claims = claims_from_display(response.display_claims);
    Ok(XboxTokenWithClaims {
        token: token_from_response(TokenResponse {
            token: response.token,
            display_claims: DisplayClaims::default(),
        })?,
        claims,
    })
}

fn claims_from_display(display: DisplayClaims) -> XboxClaims {
    let Some(values) = display.xui.first() else {
        return XboxClaims::default();
    };
    let string = |name: &str| {
        values
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    XboxClaims {
        xuid: string("xid"),
        user_hash: string("uhs"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_signature_is_stable() {
        assert!(PNG_SIGNATURE == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }
}
