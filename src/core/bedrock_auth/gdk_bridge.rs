use std::sync::Mutex;

const MAX_LAUNCH_PREAUTH_SIZE: usize = 256 * 1024;
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
