use super::{AuthError, XboxProfile};
use base64::Engine as _;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::rand_core::OsRng;
use secrecy::{ExposeSecret as _, SecretSlice, SecretString};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const DEVICE_AUTH_ENDPOINT: &str = "https://device.auth.xboxlive.com/device/authenticate";
const USER_AUTH_ENDPOINT: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_ENDPOINT: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const SISU_ENDPOINT: &str = "https://sisu.xboxlive.com/authorize";
const PROFILE_ENDPOINT: &str = "https://profile.xboxlive.com/users/batch/profile/settings";
const PROFILE_RELYING_PARTY: &str = "http://xboxlive.com";
const PLAYFAB_RELYING_PARTY: &str = "https://b980a380.minecraft.playfabapi.com/";
const MULTIPLAYER_RELYING_PARTY: &str = "https://multiplayer.minecraft.net/";
const REALMS_RELYING_PARTY: &str = "https://pocket.realms.minecraft.net/";
const LICENSING_RELYING_PARTY: &str = "http://licensing.xboxlive.com";
const WINDOWS_FILE_TIME_EPOCH_OFFSET_SECONDS: u64 = 11_644_473_600;

pub(super) struct XboxPreauth {
    pub(super) profile: XboxProfile,
    device_id: String,
    private_key_blob: SecretSlice<u8>,
    device: Token,
    user: Token,
    profile_token: TokenWithClaims,
    achievements: Option<TokenWithClaims>,
    playfab: TokenWithClaims,
    multiplayer: TokenWithClaims,
    realms: TokenWithClaims,
    licensing: Option<TokenWithClaims>,
}

impl std::fmt::Debug for XboxPreauth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XboxPreauth")
            .field("profile", &self.profile)
            .field("device_id", &self.device_id)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug)]
struct DeviceIdentity {
    id: String,
    signing_key: SigningKey,
}

#[derive(Debug)]
struct Token {
    value: SecretString,
    not_after: String,
}

#[derive(Debug)]
struct TokenWithClaims {
    token: Token,
    claims: XboxClaims,
}

#[derive(Clone, Debug, Default)]
struct XboxClaims {
    xuid: Option<String>,
    gamertag: Option<String>,
    user_hash: Option<String>,
    age_group: Option<String>,
    modern_gamertag: Option<String>,
    modern_gamertag_suffix: Option<String>,
    unique_modern_gamertag: Option<String>,
    privileges: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "NotAfter", default)]
    not_after: String,
    #[serde(rename = "DisplayClaims", default)]
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct SisuResponse {
    #[serde(rename = "AuthorizationToken")]
    authorization_token: TokenResponse,
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
    settings: [&'static str; 4],
}

pub(super) async fn authenticate(
    client: &reqwest::Client,
    msa_access_token: &SecretString,
) -> Result<XboxPreauth, AuthError> {
    let identity = load_or_create_device_identity().await?;
    let proof_key = proof_key(&identity.signing_key)?;

    let device_response: TokenResponse = post_signed_json(
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
    )
    .await?;
    let device = token_from_response(device_response)?;

    let user_response: TokenResponse = post_signed_json(
        client,
        &identity.signing_key,
        USER_AUTH_ENDPOINT,
        "",
        &json!({
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("t={}", msa_access_token.expose_secret()),
            }
        }),
        "user-auth",
    )
    .await?;
    let user = token_from_response(user_response)?;

    let achievements = xsts(
        client,
        &identity.signing_key,
        &user,
        PROFILE_RELYING_PARTY,
        "xsts-achievements",
    )
    .await
    .map_err(|error| {
        tracing::warn!(%error, "optional Xbox achievements token was unavailable");
        error
    })
    .ok();

    let profile_token = sisu(
        client,
        &identity.signing_key,
        msa_access_token,
        &device,
        &proof_key,
        PROFILE_RELYING_PARTY,
        "sisu-profile",
    )
    .await?;
    let playfab = sisu(
        client,
        &identity.signing_key,
        msa_access_token,
        &device,
        &proof_key,
        PLAYFAB_RELYING_PARTY,
        "sisu-playfab",
    )
    .await?;
    let multiplayer = sisu(
        client,
        &identity.signing_key,
        msa_access_token,
        &device,
        &proof_key,
        MULTIPLAYER_RELYING_PARTY,
        "sisu-multiplayer",
    )
    .await?;
    let realms = sisu(
        client,
        &identity.signing_key,
        msa_access_token,
        &device,
        &proof_key,
        REALMS_RELYING_PARTY,
        "sisu-realms",
    )
    .await?;
    let licensing = sisu(
        client,
        &identity.signing_key,
        msa_access_token,
        &device,
        &proof_key,
        LICENSING_RELYING_PARTY,
        "sisu-licensing",
    )
    .await
    .map_err(|error| {
        tracing::warn!(%error, "optional Xbox licensing token was unavailable");
        error
    })
    .ok();

    let xuid = profile_token
        .claims
        .xuid
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::InvalidResponse("Xbox profile token 缺少 XUID"))?;
    let profile = fetch_profile(client, &identity.signing_key, xuid, &profile_token).await?;
    let private_key_blob = SecretSlice::from(bcrypt_private_blob(&identity.signing_key)?);

    Ok(XboxPreauth {
        profile,
        device_id: identity.id,
        private_key_blob,
        device,
        user,
        profile_token,
        achievements,
        playfab,
        multiplayer,
        realms,
        licensing,
    })
}

async fn load_or_create_device_identity() -> Result<DeviceIdentity, AuthError> {
    crate::tasks::runtime::run_io_blocking(|| {
        let _guard = super::ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Xbox 设备身份锁已损坏".to_string())?;
        let stored_key = super::secret_store::load_device_private_key()?;
        let signing_key = match stored_key {
            Some(bytes) => SigningKey::from_slice(bytes.expose_secret())
                .map_err(|_| "系统凭证存储中的 Xbox 设备密钥无效".to_string())?,
            None => {
                let signing_key = SigningKey::random(&mut OsRng);
                let secret = SecretSlice::from(signing_key.to_bytes().to_vec());
                super::secret_store::store_device_private_key(&secret)?;
                signing_key
            }
        };

        let auth_dir = crate::utils::file_ops::state_subdir("bedrock-auth");
        std::fs::create_dir_all(&auth_dir)
            .map_err(|error| format!("创建 Xbox 设备状态目录失败：{error}"))?;
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&auth_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("限制 Xbox 设备状态目录权限失败：{error}"))?;
        let device_id_path = auth_dir.join("device-id");
        let id = match std::fs::read_to_string(&device_id_path) {
            Ok(value) if valid_device_id(value.trim()) => value.trim().to_string(),
            Ok(_) | Err(_) => {
                let value = format!("{{{}}}", uuid::Uuid::new_v4());
                let temporary = auth_dir.join(".device-id.tmp");
                std::fs::write(&temporary, format!("{value}\n"))
                    .map_err(|error| format!("写入 Xbox 设备 ID 失败：{error}"))?;
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
                    .map_err(|error| format!("限制 Xbox 设备 ID 权限失败：{error}"))?;
                std::fs::rename(&temporary, &device_id_path)
                    .map_err(|error| format!("保存 Xbox 设备 ID 失败：{error}"))?;
                value
            }
        };
        Ok(DeviceIdentity { id, signing_key })
    })
    .await
    .map_err(AuthError::Runtime)?
    .map_err(AuthError::Storage)
}

fn valid_device_id(value: &str) -> bool {
    value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some()
}

fn proof_key(signing_key: &SigningKey) -> Result<Value, AuthError> {
    let encoded = signing_key.verifying_key().to_encoded_point(false);
    let x = encoded
        .x()
        .ok_or(AuthError::InvalidResponse("P-256 公钥缺少 X 坐标"))?;
    let y = encoded
        .y()
        .ok_or(AuthError::InvalidResponse("P-256 公钥缺少 Y 坐标"))?;
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

async fn xsts(
    client: &reqwest::Client,
    signing_key: &SigningKey,
    user: &Token,
    relying_party: &str,
    stage: &'static str,
) -> Result<TokenWithClaims, AuthError> {
    let response: TokenResponse = post_signed_json(
        client,
        signing_key,
        XSTS_ENDPOINT,
        "",
        &json!({
            "RelyingParty": relying_party,
            "TokenType": "JWT",
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [user.value.expose_secret()],
            }
        }),
        stage,
    )
    .await?;
    token_with_claims(response)
}

async fn sisu(
    client: &reqwest::Client,
    signing_key: &SigningKey,
    msa_access_token: &SecretString,
    device: &Token,
    proof_key: &Value,
    relying_party: &str,
    stage: &'static str,
) -> Result<TokenWithClaims, AuthError> {
    let response: SisuResponse = post_signed_json(
        client,
        signing_key,
        SISU_ENDPOINT,
        "",
        &json!({
            "AccessToken": format!("t={}", msa_access_token.expose_secret()),
            "AppId": "0000000048183522",
            "deviceToken": device.value.expose_secret(),
            "Sandbox": "RETAIL",
            "UseModernGamertag": true,
            "SiteName": "user.auth.xboxlive.com",
            "RelyingParty": relying_party,
            "OfferTermsAcceptance": true,
            "AcceptOffers": true,
            "ProofKey": proof_key,
        }),
        stage,
    )
    .await?;
    token_with_claims(response.authorization_token)
}

async fn fetch_profile(
    client: &reqwest::Client,
    signing_key: &SigningKey,
    xuid: &str,
    token: &TokenWithClaims,
) -> Result<XboxProfile, AuthError> {
    let user_hash = token
        .claims
        .user_hash
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::InvalidResponse(
            "Xbox profile token 缺少 user hash",
        ))?;
    let authorization = format!("XBL3.0 x={user_hash};{}", token.token.value.expose_secret());
    let response: ProfileResponse = post_signed_json(
        client,
        signing_key,
        PROFILE_ENDPOINT,
        &authorization,
        &ProfileRequest {
            user_ids: [xuid],
            settings: [
                "GameDisplayName",
                "GameDisplayPicRaw",
                "Gamerscore",
                "Gamertag",
            ],
        },
        "xbox-profile",
    )
    .await?;
    let user = response
        .profile_users
        .into_iter()
        .find(|user| user.id == xuid)
        .ok_or(AuthError::InvalidResponse(
            "Xbox Profile API 未返回当前用户",
        ))?;
    let settings = user
        .settings
        .into_iter()
        .map(|setting| (setting.id, setting.value))
        .collect::<HashMap<_, _>>();
    let gamertag = settings
        .get("Gamertag")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| token.claims.gamertag.clone())
        .ok_or(AuthError::InvalidResponse("Xbox Profile API 缺少 gamertag"))?;
    let display_name = settings
        .get("GameDisplayName")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| gamertag.clone());
    let avatar_url = settings
        .get("GameDisplayPicRaw")
        .filter(|value| value.starts_with("https://"))
        .map(|value| gamerpic_url(value));
    Ok(XboxProfile {
        xuid: xuid.to_string(),
        gamertag,
        display_name,
        gamerscore: settings.get("Gamerscore").cloned(),
        avatar_url,
    })
}

fn gamerpic_url(raw: &str) -> String {
    if raw.contains("&format=") {
        raw.to_string()
    } else {
        format!("{raw}&format=png&w=208&h=208")
    }
}

async fn post_signed_json<T, B>(
    client: &reqwest::Client,
    signing_key: &SigningKey,
    endpoint: &'static str,
    authorization: &str,
    body: &B,
    stage: &'static str,
) -> Result<T, AuthError>
where
    T: DeserializeOwned,
    B: Serialize + ?Sized,
{
    let body = serde_json::to_vec(body)
        .map_err(|error| AuthError::Protocol(format!("{stage}: JSON 编码失败：{error}")))?;
    let url = reqwest::Url::parse(endpoint)
        .map_err(|error| AuthError::Protocol(format!("{stage}: URL 无效：{error}")))?;
    let path_and_query = url.query().map_or_else(
        || url.path().to_string(),
        |query| format!("{}?{query}", url.path()),
    );
    let signature = signature_header(signing_key, "POST", &path_and_query, authorization, &body)?;
    let mut request = client
        .post(url)
        .header(
            reqwest::header::USER_AGENT,
            "XAL Xbox Live Game (Windows; SDK; 1.0.0.0)",
        )
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
    let response = request.send().await.map_err(AuthError::Http)?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(AuthError::Http)?;
    if !status.is_success() {
        let error_code = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|payload| xbox_error_code(&payload));
        return Err(AuthError::XboxService {
            stage,
            status: status.as_u16(),
            error_code,
        });
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AuthError::InvalidResponse("Xbox 服务返回了无效 JSON"))
}

fn xbox_error_code(payload: &Value) -> Option<u64> {
    payload
        .get("XErr")
        .or_else(|| payload.get("xerr"))
        .or_else(|| payload.get("XErrCode"))
        .and_then(|value| {
            value.as_u64().or_else(|| {
                value.as_str().and_then(|value| {
                    value.strip_prefix("0x").map_or_else(
                        || value.parse().ok(),
                        |value| u64::from_str_radix(value, 16).ok(),
                    )
                })
            })
        })
}

fn signature_header(
    signing_key: &SigningKey,
    method: &str,
    path_and_query: &str,
    authorization: &str,
    body: &[u8],
) -> Result<String, AuthError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::Protocol("系统时间早于 Unix epoch".to_string()))?
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

fn token_from_response(response: TokenResponse) -> Result<Token, AuthError> {
    if response.token.is_empty() {
        return Err(AuthError::InvalidResponse("Xbox 服务响应缺少 token"));
    }
    if response.not_after.is_empty() {
        return Err(AuthError::InvalidResponse("Xbox 服务响应缺少过期时间"));
    }
    Ok(Token {
        value: SecretString::from(response.token),
        not_after: response.not_after,
    })
}

fn token_with_claims(response: TokenResponse) -> Result<TokenWithClaims, AuthError> {
    let TokenResponse {
        token,
        not_after,
        display_claims,
    } = response;
    let claims = claims_from_display(display_claims);
    Ok(TokenWithClaims {
        token: token_from_response(TokenResponse {
            token,
            not_after,
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
        gamertag: string("gtg"),
        user_hash: string("uhs"),
        age_group: string("agg"),
        modern_gamertag: string("mgt"),
        modern_gamertag_suffix: string("mgs"),
        unique_modern_gamertag: string("umg"),
        privileges: normalize_privileges(values.get("prv")),
    }
}

fn normalize_privileges(value: Option<&Value>) -> Option<String> {
    let mut privileges = value?
        .as_str()?
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter_map(|value| value.parse::<u32>().ok())
        .collect::<Vec<_>>();
    privileges.sort_unstable();
    privileges.dedup();
    (!privileges.is_empty()).then(|| {
        privileges
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn bcrypt_private_blob(signing_key: &SigningKey) -> Result<Vec<u8>, AuthError> {
    let encoded = signing_key.verifying_key().to_encoded_point(false);
    let x = encoded
        .x()
        .ok_or(AuthError::InvalidResponse("P-256 公钥缺少 X 坐标"))?;
    let y = encoded
        .y()
        .ok_or(AuthError::InvalidResponse("P-256 公钥缺少 Y 坐标"))?;
    let private = signing_key.to_bytes();
    let mut blob = Vec::with_capacity(104);
    blob.extend_from_slice(&0x3253_4345_u32.to_le_bytes());
    blob.extend_from_slice(&32_u32.to_le_bytes());
    blob.extend_from_slice(x);
    blob.extend_from_slice(y);
    blob.extend_from_slice(&private);
    Ok(blob)
}

impl XboxPreauth {
    pub(super) fn winegdk_json(&self) -> Result<Vec<u8>, AuthError> {
        let claims = &self.profile_token.claims;
        let mut value = json!({
            "device_id": self.device_id,
            "ecc_private_blob_b64": base64::engine::general_purpose::STANDARD
                .encode(self.private_key_blob.expose_secret()),
            "device_token": self.device.value.expose_secret(),
            "device_token_expiry": self.device.not_after,
            "user_token": self.user.value.expose_secret(),
            "user_token_expiry": self.user.not_after,
            "user_token_expiry_epoch": expiry_epoch(&self.user.not_after)?,
            "xbl_token": self.profile_token.token.value.expose_secret(),
            "xbl_token_expiry": self.profile_token.token.not_after,
            "xbl_token_expiry_epoch": expiry_epoch(&self.profile_token.token.not_after)?,
            "xbl_xuid": claims.xuid,
            "xbl_gamertag": claims.gamertag,
            "xbl_age_group": claims.age_group,
            "xbl_uhs": claims.user_hash,
            "xbl_modern_gamertag": claims.modern_gamertag,
            "xbl_modern_gamertag_suffix": claims.modern_gamertag_suffix,
            "xbl_unique_modern_gamertag": claims.unique_modern_gamertag,
            "xbl_privileges": claims.privileges,
            "sisu_rp": PLAYFAB_RELYING_PARTY,
            "sisu_token": self.playfab.token.value.expose_secret(),
            "sisu_uhs": self.playfab.claims.user_hash,
            "sisu_expiry": self.playfab.token.not_after,
            "sisu_expiry_epoch": expiry_epoch(&self.playfab.token.not_after)?,
            "mp_rp": MULTIPLAYER_RELYING_PARTY,
            "mp_token": self.multiplayer.token.value.expose_secret(),
            "mp_uhs": self.multiplayer.claims.user_hash,
            "mp_expiry": self.multiplayer.token.not_after,
            "mp_expiry_epoch": expiry_epoch(&self.multiplayer.token.not_after)?,
            "realms_rp": REALMS_RELYING_PARTY,
            "realms_token": self.realms.token.value.expose_secret(),
            "realms_uhs": self.realms.claims.user_hash,
            "realms_expiry": self.realms.token.not_after,
            "realms_expiry_epoch": expiry_epoch(&self.realms.token.not_after)?,
            "obtained": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| AuthError::Protocol("系统时间早于 Unix epoch".to_string()))?
                .as_secs(),
        });
        if let Some(achievements) = &self.achievements {
            value["achievements_token"] =
                Value::String(achievements.token.value.expose_secret().to_string());
            value["achievements_uhs"] =
                Value::String(achievements.claims.user_hash.clone().unwrap_or_default());
            value["achievements_expiry"] = Value::String(achievements.token.not_after.clone());
            value["achievements_expiry_epoch"] =
                Value::String(expiry_epoch(&achievements.token.not_after)?);
        }
        if let Some(licensing) = &self.licensing {
            value["lic_rp"] = Value::String(LICENSING_RELYING_PARTY.to_string());
            value["lic_token"] = Value::String(licensing.token.value.expose_secret().to_string());
            value["lic_uhs"] =
                Value::String(licensing.claims.user_hash.clone().unwrap_or_default());
            value["lic_expiry"] = Value::String(licensing.token.not_after.clone());
            value["lic_expiry_epoch"] = Value::String(expiry_epoch(&licensing.token.not_after)?);
        }
        serde_json::to_vec(&value)
            .map_err(|error| AuthError::Protocol(format!("编码 WineGDK 预认证失败：{error}")))
    }
}

fn expiry_epoch(value: &str) -> Result<String, AuthError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.timestamp().to_string())
        .map_err(|_| AuthError::InvalidResponse("Xbox token 过期时间无效"))
}

#[cfg(test)]
mod tests {
    use super::{gamerpic_url, normalize_privileges, valid_device_id};
    use serde_json::json;

    #[test]
    fn device_id_requires_braced_uuid() {
        assert!(valid_device_id("{f1db3f85-8ff3-49ce-bb72-bab2fbe00ac8}"));
        assert!(!valid_device_id("f1db3f85-8ff3-49ce-bb72-bab2fbe00ac8"));
    }

    #[test]
    fn gamerpic_url_requests_supported_size() {
        assert_eq!(
            gamerpic_url("https://images.example/image?url=avatar"),
            "https://images.example/image?url=avatar&format=png&w=208&h=208"
        );
    }

    #[test]
    fn privileges_are_sorted_and_deduplicated() {
        assert_eq!(
            normalize_privileges(Some(&json!("254 185 254"))),
            Some("185 254".to_string())
        );
    }
}
