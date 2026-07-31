const MAX_LAUNCH_PREAUTH_SIZE: usize = 256 * 1024;

pub(crate) struct PreparedLaunchAuth {
    payload: Vec<u8>,
    pub(crate) gamertag: String,
}

impl PreparedLaunchAuth {
    /// Transfers the short-lived pre-authentication document to the Win32
    /// launcher path. The caller must hand it only to BLoader's process-scoped
    /// named-pipe server and must never log or persist the bytes.
    pub(crate) fn into_payload(mut self) -> Vec<u8> {
        std::mem::take(&mut self.payload)
    }
}

impl Drop for PreparedLaunchAuth {
    fn drop(&mut self) {
        self.payload.fill(0);
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
        payload: device_json.to_vec(),
        gamertag: gamertag.to_string(),
    })
}
