use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LAUNCH_PREAUTH_SIZE: usize = 256 * 1024;
const MIN_USER_TOKEN_REMAINING_SECONDS: u64 = 30;
const AUTH_MODE: &str = "hybrid-native-or-bmcbl-token-v1";
// Process-local metadata key used only between BMCBL modules. This key and its
// opaque value are consumed before CreateProcessW and are never added to the
// Minecraft child-process environment.
const INTERNAL_SESSION_KEY: &str = "BMCBL_XGAMERUNTIME_PREAUTH";

pub(crate) struct PreparedLaunchAuth {
    payload: Mutex<Option<Vec<u8>>>,
    pub(crate) gamertag: String,
}

impl PreparedLaunchAuth {
    /// Moves the credential payload into BMCBL's process-local one-use
    /// registry and returns only an opaque handle for the launcher module.
    /// Neither the handle nor the credential payload is passed through the
    /// Minecraft environment, command line, registry, or a temporary file.
    pub(crate) fn take_secure_launch_metadata(&self) -> Vec<(String, String)> {
        let payload = self
            .payload
            .lock()
            .ok()
            .and_then(|mut payload| payload.take())
            .unwrap_or_default();
        if payload.is_empty() {
            return Vec::new();
        }
        let handle = crate::core::inject::inject::register_xuser_launch_payload(payload);
        vec![(INTERNAL_SESSION_KEY.to_string(), handle)]
    }
}

impl Drop for PreparedLaunchAuth {
    fn drop(&mut self) {
        if let Ok(payload) = self.payload.get_mut()
            && let Some(payload) = payload.as_mut()
        {
            payload.fill(0);
        }
    }
}

/// Produces the Windows BLoader launch credential envelope.
///
/// Two routes are carried in one process-scoped, read-once payload:
/// - same-account: BLoader delegates token/signature acquisition to the
///   Microsoft Gaming Runtime and ignores the fallback credentials;
/// - cross-account: until a verified pre-XSTS UToken injection boundary is
///   available in the Microsoft runtime, BLoader may use BMCBL's already
///   authenticated service XSTS tokens and PoP signing key as a compatibility
///   fallback. The MSA refresh token never crosses into Minecraft.
pub(super) fn prepare(
    profile_id: &str,
    gamertag: &str,
    device_json: &[u8],
) -> Result<PreparedLaunchAuth, String> {
    if device_json.is_empty() {
        return Err("GDK 预认证数据为空".to_string());
    }
    if device_json.len() > MAX_LAUNCH_PREAUTH_SIZE {
        return Err("GDK 预认证数据超过安全传输上限".to_string());
    }
    if profile_id.is_empty() || gamertag.trim().is_empty() {
        return Err("Xbox 用户身份为空".to_string());
    }

    let mut payload: Value = serde_json::from_slice(device_json)
        .map_err(|_| "GDK 预认证数据不是有效 JSON".to_string())?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "GDK 预认证数据必须是 JSON object".to_string())?;

    let user_token = object
        .get("user_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GDK 预认证数据缺少原始 Xbox UToken".to_string())?;
    let user_token_expiry_epoch = object
        .get("user_token_expiry_epoch")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "GDK 预认证数据缺少有效 UToken 过期时间".to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if user_token_expiry_epoch <= now.saturating_add(MIN_USER_TOKEN_REMAINING_SECONDS) {
        return Err("Xbox UToken 已过期或即将过期".to_string());
    }

    // The fallback route requires the same fields used by the previously
    // working BLoader pre-auth token provider. Validate them before launching
    // so a cross-account game never degrades into an identity-only session.
    for key in [
        "ecc_private_blob_b64",
        "xbl_token",
        "xbl_uhs",
        "xbl_token_expiry_epoch",
        "sisu_token",
        "sisu_uhs",
        "sisu_expiry_epoch",
        "mp_token",
        "mp_uhs",
        "mp_expiry_epoch",
        "realms_token",
        "realms_uhs",
        "realms_expiry_epoch",
    ] {
        if object
            .get(key)
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty())
        {
            return Err(format!("GDK 预认证数据缺少跨账号回退字段：{key}"));
        }
    }

    object.insert("auth_mode".to_string(), Value::String(AUTH_MODE.to_string()));
    object.insert("xbl_xuid".to_string(), Value::String(profile_id.to_string()));
    object.insert(
        "xbl_gamertag".to_string(),
        Value::String(gamertag.to_string()),
    );
    object.insert(
        "user_token_expiry_epoch".to_string(),
        Value::String(user_token_expiry_epoch.to_string()),
    );
    // Keep an explicit marker so diagnostics can distinguish the compatibility
    // route without exposing any token material.
    object.insert(
        "cross_account_fallback".to_string(),
        json!("bmcbl-preauth-v1"),
    );

    let payload = serde_json::to_vec(&payload)
        .map_err(|error| format!("编码 Xbox 混合启动载荷失败：{error}"))?;
    if payload.len() > MAX_LAUNCH_PREAUTH_SIZE {
        return Err("Xbox 混合启动载荷超过安全传输上限".to_string());
    }

    Ok(PreparedLaunchAuth {
        payload: Mutex::new(Some(payload)),
        gamertag: gamertag.to_string(),
    })
}
