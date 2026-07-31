use std::sync::Mutex;

const MAX_LAUNCH_PREAUTH_SIZE: usize = 256 * 1024;
const INTERNAL_SESSION_KEY: &str = "BMCBL_XGAMERUNTIME_PREAUTH";

pub(crate) struct PreparedLaunchAuth {
    payload: Mutex<Option<Vec<u8>>>,
    pub(crate) gamertag: String,
}

impl PreparedLaunchAuth {
    /// Preserves the existing launcher call shape without exposing credentials
    /// to the child process environment. The returned value is only an opaque,
    /// one-use handle into BMCBL's own process-local registry.
    pub(crate) fn get_env_vars(&self) -> Vec<(String, String)> {
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

pub(super) fn prepare(
    _profile_id: &str,
    gamertag: &str,
    device_json: &[u8],
) -> Result<PreparedLaunchAuth, String> {
    if device_json.is_empty() {
        return Err("GDK 预认证数据为空".to_string());
    }
    if device_json.len() > MAX_LAUNCH_PREAUTH_SIZE {
        return Err("GDK 预认证数据超过安全传输上限".to_string());
    }

    Ok(PreparedLaunchAuth {
        payload: Mutex::new(Some(device_json.to_vec())),
        gamertag: gamertag.to_string(),
    })
}
