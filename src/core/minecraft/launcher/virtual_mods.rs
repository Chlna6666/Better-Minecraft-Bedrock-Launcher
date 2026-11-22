// SPDX-FileCopyrightText: 2026 Chlna6666 and BMCBL contributors
// SPDX-License-Identifier: GPL-3.0-only
// BMCBL upstream: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher
// AI/code-reuse attribution: preserve upstream credit and license notices when
// reproducing or adapting copyrighted code. See the repository NOTICE.md.
//! Authenticated, per-game BMCBL -> BLoader virtual Mod handoff.
//! Only the launcher constructs the plan; the DLLs remain at
//! BMCBL/plugins/<plugin_id>/game-mods/*.dll and are never copied.
mod rpc;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) use crate::plugins::game_mods::LaunchPlan;
pub(crate) use rpc::{call as call_native, take_events};
#[cfg(target_os = "linux")]
pub(crate) use linux::prepare as prepare_linux;
#[cfg(target_os = "windows")]
pub(crate) use windows::start as start_windows;

const PROTOCOL_MAJOR: u64 = 1;
const MAX_FRAME: usize = 65_536;
const MAX_STATUS_SESSIONS: usize = 16;

#[derive(Clone, Debug)]
pub(crate) struct ModuleStatus {
    pub session_id: String,
    pub module: String,
    pub phase: String,
    pub state: String,
    pub rpc_available: bool,
    pub error_code: Option<String>,
}

static REPORTS: OnceLock<Mutex<BTreeMap<String, ModuleStatus>>> = OnceLock::new();

pub(crate) fn statuses() -> Vec<ModuleStatus> {
    REPORTS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .values()
        .cloned()
        .collect()
}

fn record_status(session: &str, value: &Value) {
    let Some(module) = value.get("module").and_then(Value::as_str) else {
        return;
    };
    let Some(state) = value.get("state").and_then(Value::as_str) else {
        return;
    };
    if module.len() > 130 || state.len() > 32 {
        return;
    }
    let report = ModuleStatus {
        session_id: session.to_owned(),
        module: module.to_owned(),
        phase: value.get("phase").and_then(Value::as_str).unwrap_or("").to_owned(),
        state: state.to_owned(),
        rpc_available: value.get("rpc_available").and_then(Value::as_bool).unwrap_or(false),
        error_code: value.get("error_code").and_then(Value::as_str).map(ToOwned::to_owned),
    };
    tracing::info!(
        module = %report.module,
        phase = %report.phase,
        state = %report.state,
        rpc_available = report.rpc_available,
        error_code = ?report.error_code,
        "BLoader virtual Mod module status"
    );
    let mut reports = REPORTS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if reports.len() >= 256 * MAX_STATUS_SESSIONS {
        let oldest = reports.keys().next().cloned();
        if let Some(oldest) = oldest {
            reports.remove(&oldest);
        }
    }
    reports.insert(format!("{session}/{module}"), report);
}

fn write_frame<W: Write>(stream: &mut W, message: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(message).map_err(|error| format!("serialize Mod IPC: {error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err("virtual Mod IPC plan exceeds the 64 KiB frame budget".into());
    }
    stream.write_all(&(bytes.len() as u32).to_le_bytes()).map_err(|error| error.to_string())?;
    stream.write_all(&bytes).map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn read_frame<R: Read>(stream: &mut R) -> Result<Value, String> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).map_err(|error| error.to_string())?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err("invalid virtual Mod IPC frame length".into());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).map_err(|error| error.to_string())?;
    serde_json::from_slice(&body).map_err(|error| format!("invalid Mod IPC JSON: {error}"))
}

fn check_seq(frame: &Value, last: u64) -> Result<u64, String> {
    let text = frame.get("seq").and_then(Value::as_str)
        .ok_or("Mod IPC message missing seq")?;
    if text.len() != 16 ||
        !text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Mod IPC seq must be 16 lowercase hex digits".into());
    }
    let number = u64::from_str_radix(text, 16).map_err(|error| error.to_string())?;
    if number <= last {
        return Err("replayed or out-of-order Mod IPC sequence".into());
    }
    Ok(number)
}

fn serve<T: Read + Write, W: Write + Send + 'static>(
    stream: &mut T,
    outgoing: W,
    plan: &LaunchPlan,
    plugins_root_windows: &str,
    transport: &str,
    expected_guest_pid: Option<u32>,
    session_id: &str,
    game_version: &str,
) -> Result<(), String> {
    let hello = read_frame(stream)?;
    if hello.get("type").and_then(Value::as_str) != Some("hello")
        || hello.get("protocol_major").and_then(Value::as_u64) != Some(PROTOCOL_MAJOR)
        || hello.get("transport").and_then(Value::as_str) != Some(transport)
        || hello.get("max_frame_bytes").and_then(Value::as_u64).is_none_or(|n| n < 256)
    {
        return Err("virtual Mod hello protocol or transport mismatch".into());
    }
    if let Some(expected) = expected_guest_pid {
        if hello.get("target_pid").and_then(Value::as_u64) != Some(u64::from(expected)) {
            return Err("virtual Mod client PID differs from launch target".into());
        }
    }
    if check_seq(&hello, 0)? != 1 {
        return Err("virtual Mod hello sequence must start at 1".into());
    }

    // The per-launch UUID is not a credential. Authorization of the
    // transport has already been established by Windows process identity
    // or Linux PSK mutual authentication.
    write_frame(stream, &json!({
        "type": "welcome",
        "seq": "0000000000000001",
        "session_id": session_id,
        "protocol_major": PROTOCOL_MAJOR,
        "protocol_minor": 0,
        "max_frame_bytes": MAX_FRAME,
        "game_version": game_version,
        "game_instance_id": session_id
    }))?;
    write_frame(stream, &json!({
        "type": "launch_plan",
        "seq": "0000000000000002",
        "session_id": session_id,
        "plugins_root": plugins_root_windows,
        "modules": plan.modules
    }))?;
    let ack = read_frame(stream)?;
    if ack.get("type").and_then(Value::as_str) != Some("plan_ack")
        || ack.get("session_id").and_then(Value::as_str) != Some(session_id)
        || check_seq(&ack, 1)? != 2
    {
        return Err("virtual Mod launch plan not acknowledged".into());
    }
    tracing::info!(session_id, modules = plan.modules.len(), transport,
        "BLoader accepted virtual Mod launch plan");

    let rpc_session = rpc::register(session_id, plan, outgoing)?;
    let mut last = 2_u64;
    // Each game owns a background receiver. No polling of .log files.
    let outcome = (|| -> Result<(), String> {
    loop {
        let frame = match read_frame(stream) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!(session_id, %error, "virtual Mod IPC closed");
                break;
            }
        };
        last = check_seq(&frame, last)?;
        if frame.get("session_id").and_then(Value::as_str) != Some(session_id) {
            return Err("cross-session virtual Mod IPC packet".into());
        }
        match frame.get("type").and_then(Value::as_str) {
            Some("module_status") => {
                rpc_session.observe_status(&frame);
                record_status(session_id, &frame);
            }
            Some("rpc_response") => rpc_session.handle_response(&frame)?,
            Some("mod_event") => rpc_session.handle_event(&frame)?,
            Some("error") => {
                tracing::warn!(session_id,
                    error_code = ?frame.get("error_code").and_then(|value| value.as_str()),
                    "BLoader virtual Mod protocol error");
            }
            _ => return Err("unexpected virtual Mod IPC message".into()),
        }
    }
    Ok(())
    })();
    rpc_session.close();
    outcome
}

fn valid_rpc_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric()
            || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn plan_for_launch(version_id: &str, game_version: &str) -> Result<Option<LaunchPlan>, String> {
    crate::plugins::game_mods::launch_plan(version_id, game_version)
}

pub(crate) fn new_session_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_is_length_bounded() {
        let mut output = Vec::new();
        write_frame(&mut output, &json!({"type":"hello","seq":"0000000000000001"}))
            .expect("small JSON frame");
        let result = read_frame(&mut output.as_slice()).expect("read frame");
        assert_eq!(result["type"], "hello");
        assert_eq!(check_seq(&result, 0).unwrap(), 1);
        assert!(check_seq(&result, 1).is_err());
        assert!(read_frame(&mut &[255_u8; 4][..]).is_err());
    }
}
