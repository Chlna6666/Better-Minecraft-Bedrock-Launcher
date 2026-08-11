use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LAUNCH_PREAUTH_SIZE: usize = 256 * 1024;
const MIN_USER_TOKEN_REMAINING_SECONDS: u64 = 30;
const AUTH_MODE: &str = "official-runtime-user-token-v4";
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
/// BMCBL owns Microsoft refresh credentials and performs the user login. The
/// Minecraft process receives only the selected user's raw Xbox XASU UToken
/// plus public XUser metadata. `native_system_xuid_hint` is also public metadata
/// and exists only to gate an optional same-account silent native XUser add.
/// DeviceToken, TitleToken, final XSTS, Microsoft access/refresh tokens and HTTP
/// request signing keys intentionally do not cross this boundary.
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

    let source: Value = serde_json::from_slice(device_json)
        .map_err(|_| "GDK 预认证数据不是有效 JSON".to_string())?;
    let user_token = source
        .get("user_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GDK 预认证数据缺少原始 Xbox UToken".to_string())?;
    let user_token_expiry_epoch = source
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

    // Lightweight local-state lookup only. It does not call Gaming Runtime,
    // WAM or gamer-picture APIs and never triggers Xbox sign-in UI.
    let native_system_xuid_hint = crate::core::system_xbox_user::probe_default_user_xuid();
    let native_identity_relation = native_system_xuid_hint.map_or("none", |native_xuid| {
        if native_xuid.to_string() == profile_id {
            "same"
        } else {
            "different"
        }
    });
    tracing::debug!(
        custom_xuid = %profile_id,
        native_system_xuid_hint = ?native_system_xuid_hint,
        relation = native_identity_relation,
        "已生成 BLoader native XUser 安全路由提示"
    );

    let payload = json!({
        "auth_mode": AUTH_MODE,
        "xbl_xuid": profile_id,
        "xbl_gamertag": gamertag,
        "xbl_age_group": source.get("xbl_age_group").cloned().unwrap_or(Value::Null),
        "xbl_privileges": source.get("xbl_privileges").cloned().unwrap_or(Value::Null),
        "user_token": user_token,
        "user_token_expiry_epoch": user_token_expiry_epoch.to_string(),
        "native_system_xuid_hint": native_system_xuid_hint.map(|value| value.to_string()),
    });
    let payload = serde_json::to_vec(&payload)
        .map_err(|error| format!("编码 Xbox UToken 启动载荷失败：{error}"))?;
    if payload.len() > MAX_LAUNCH_PREAUTH_SIZE {
        return Err("Xbox UToken 启动载荷超过安全传输上限".to_string());
    }

    Ok(PreparedLaunchAuth {
        payload: Mutex::new(Some(payload)),
        gamertag: gamertag.to_string(),
    })
}
