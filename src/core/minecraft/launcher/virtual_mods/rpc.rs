//! Permission-checked in-process routing for WASM -> its own authorized native
//! game Mod. The plugin never obtains the pipe, guest PID, session key or path.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, SyncSender},
};
use std::thread;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{LaunchPlan, write_frame};

const MAX_PENDING: usize = 256;
const MAX_PER_MODULE: usize = 32;
const MAX_BODY: usize = 32 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

static SESSIONS: OnceLock<Mutex<BTreeMap<String, Arc<RpcSession>>>> = OnceLock::new();

type Reply = oneshot::Sender<Result<Vec<u8>, String>>;

struct PendingCall {
    module: String,
    generation: String,
    reply: Reply,
}

enum Command {
    Request {
        request_id: u64,
        module: String,
        generation: String,
        method: String,
        body: Vec<u8>,
    },
    Cancel {
        request_id: u64,
        module: String,
        generation: String,
    },
}

/// Every session is tied to a concrete game launch and an immutable snapshot of
/// active WASM generations. Re-activating the same plugin does not inherit RPC
/// access to an old process.
pub(super) struct RpcSession {
    pub(super) id: String,
    generations: BTreeMap<String, u64>,
    module_generations: BTreeMap<String, String>,
    ready: Mutex<BTreeSet<String>>,
    pending: Mutex<HashMap<u64, PendingCall>>,
    outgoing: SyncSender<Command>,
    next_request_id: AtomicU64,
    closed: AtomicBool,
}

fn sessions() -> &'static Mutex<BTreeMap<String, Arc<RpcSession>>> {
    SESSIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|error| error.into_inner())
}

pub(super) fn register<W: Write + Send + 'static>(
    id: &str,
    plan: &LaunchPlan,
    mut writer: W,
) -> Result<Arc<RpcSession>, String> {
    let (outgoing, commands) = mpsc::sync_channel(64);
    let session = Arc::new(RpcSession {
        id: id.to_string(),
        generations: plan.runtime_generations.clone(),
        module_generations: plan.modules.iter().map(|module| (
            format!("{}/{}", module.plugin_id, module.module_id),
            module.plugin_generation.clone(),
        )).collect(),
        ready: Mutex::new(BTreeSet::new()),
        pending: Mutex::new(HashMap::new()),
        outgoing,
        next_request_id: AtomicU64::new(1),
        closed: AtomicBool::new(false),
    });
    let weak = Arc::downgrade(&session);
    thread::Builder::new()
        .name("bmcbl-virtual-mod-writer".to_string())
        .spawn(move || {
            // The first two outbound sequence numbers belong to welcome and plan.
            let mut seq = 3u64;
            for command in commands {
                let Some(session) = weak.upgrade() else { break; };
                if session.closed.load(Ordering::Acquire) { break; }
                let message = match command {
                    Command::Request { request_id, module, generation, method, body } => json!({
                        "type": "rpc_request",
                        "seq": format!("{seq:016x}"),
                        "session_id": session.id,
                        "module": module,
                        "plugin_generation": generation,
                        "request_id": format!("{request_id:016x}"),
                        "method": method,
                        "body_b64": STANDARD.encode(body),
                    }),
                    Command::Cancel { request_id, module, generation } => json!({
                        "type": "rpc_cancel",
                        "seq": format!("{seq:016x}"),
                        "session_id": session.id,
                        "module": module,
                        "plugin_generation": generation,
                        "request_id": format!("{request_id:016x}"),
                    }),
                };
                seq = seq.saturating_add(1);
                if let Err(error) = write_frame(&mut writer, &message) {
                    tracing::warn!(session_id=%session.id, %error, "virtual Mod writer disconnected");
                    session.close();
                    break;
                }
            }
        }).map_err(|error| format!("spawn Mod IPC writer: {error}"))?;
    {
        let mut sessions = lock(sessions());
        // Bound the number of live session records, rejecting new sessions
        // rather than evicting another running Minecraft instance.
        if sessions.len() >= 16 {
            session.close();
            return Err("too many concurrent Minecraft virtual Mod sessions".into());
        }
        sessions.insert(id.to_string(), session.clone());
    }
    Ok(session)
}

impl RpcSession {
    pub(super) fn observe_status(&self, frame: &Value) {
        let Some(module) = frame.get("module").and_then(Value::as_str) else { return };
        if !self.module_generations.contains_key(module) {
            return;
        }
        let loaded = frame.get("state").and_then(Value::as_str) == Some("loaded")
            && frame.get("rpc_available").and_then(Value::as_bool) == Some(true);
        let mut ready = lock(&self.ready);
        if loaded {
            ready.insert(module.to_string());
        } else {
            ready.remove(module);
        }
    }

    pub(super) fn handle_response(&self, frame: &Value) -> Result<(), String> {
        let module = frame.get("module").and_then(Value::as_str)
            .ok_or("virtual Mod response lacks module")?;
        let generation = frame.get("plugin_generation").and_then(Value::as_str)
            .ok_or("virtual Mod response lacks generation")?;
        if self.module_generations.get(module).map(String::as_str) != Some(generation) {
            return Err("virtual Mod response has wrong module generation".into());
        }
        let request = frame.get("request_id").and_then(Value::as_str)
            .ok_or("virtual Mod response lacks request ID")?;
        if request.len() != 16 {
            return Err("invalid virtual Mod request ID".into());
        }
        let request_id = u64::from_str_radix(request, 16).map_err(|_| "invalid Mod request ID")?;
        let Some(pending) = lock(&self.pending).remove(&request_id) else {
            // Cancellation and timeouts may produce late responses. These do
            // not re-open a revoked plugin's route.
            return Ok(());
        };
        if pending.module != module || pending.generation != generation {
            let _ = pending.reply.send(Err("virtual Mod response ownership mismatch".into()));
            return Err("virtual Mod response targets another plugin".into());
        }
        let reply = if frame.get("ok").and_then(Value::as_bool) == Some(true) {
            let encoded = frame.get("body_b64").and_then(Value::as_str)
                .ok_or("virtual Mod response lacks payload")?;
            if encoded.len() > 44_000 {
                return Err("virtual Mod RPC response too large".into());
            }
            let bytes = STANDARD.decode(encoded).map_err(|_| "invalid Mod response base64")?;
            if bytes.len() > MAX_BODY {
                return Err("virtual Mod RPC decoded payload too large".into());
            }
            Ok(bytes)
        } else {
            let code = frame.get("error_code").and_then(Value::as_str).unwrap_or("MODULE_UNAVAILABLE");
            Err(format!("virtual Mod RPC failed: {}", code.chars().take(64).collect::<String>()))
        };
        let _ = pending.reply.send(reply);
        Ok(())
    }

    fn cancel(&self, request_id: u64, module: &str, generation: &str) {
        lock(&self.pending).remove(&request_id);
        let _ = self.outgoing.try_send(Command::Cancel {
            request_id, module: module.to_string(), generation: generation.to_string(),
        });
    }

    pub(super) fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) { return; }
        lock(&self.ready).clear();
        for pending in std::mem::take(&mut *lock(&self.pending)).into_values() {
            let _ = pending.reply.send(Err("SESSION_CLOSED".into()));
        }
        lock(sessions()).remove(&self.id);
    }

    fn eligible(&self, plugin_id: &str, generation: u64, module: &str) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self.generations.get(plugin_id) == Some(&generation)
            && self.module_generations.contains_key(module)
            && lock(&self.ready).contains(module)
    }
}

pub(crate) async fn call(
    plugin_id: &str,
    plugin_generation: u64,
    module_id: &str,
    method: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, String> {
    if plugin_id.len() > 64
        || !super::valid_rpc_name(module_id) || !super::valid_rpc_name(method)
        || payload.len() > MAX_BODY
    {
        return Err("INVALID_MESSAGE: invalid Mod RPC target, method, or payload".into());
    }
    let full_id = format!("{plugin_id}/{module_id}");
    let matching = lock(sessions()).values()
        .filter(|session| session.eligible(plugin_id, plugin_generation, &full_id))
        .cloned()
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(if matching.is_empty() {
            "MODULE_UNAVAILABLE: no matching loaded Mod in an authorized session"
        } else {
            "AMBIGUOUS_SESSION: the Mod is loaded by multiple Minecraft games"
        }.into());
    }
    let session = &matching[0];
    let generation = session.module_generations.get(&full_id)
        .ok_or("MODULE_NOT_FOUND")?.clone();
    let id = session.next_request_id.fetch_add(1, Ordering::AcqRel);
    let (reply, response) = oneshot::channel();
    {
        let mut pending = lock(&session.pending);
        if pending.len() >= MAX_PENDING ||
            pending.values().filter(|pending| pending.module == full_id).count() >= MAX_PER_MODULE
        {
            return Err("BUSY: Mod RPC quota exceeded".into());
        }
        pending.insert(id, PendingCall {
            module: full_id.clone(), generation: generation.clone(), reply,
        });
    }
    let command = Command::Request {
        request_id: id, module: full_id.clone(), generation: generation.clone(),
        method: method.to_string(), body: payload,
    };
    if session.outgoing.try_send(command).is_err() {
        session.cancel(id, &full_id, &generation);
        return Err("BUSY: Mod IPC writer queue is unavailable".into());
    }
    match tokio::time::timeout(CALL_TIMEOUT, response).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("SESSION_CLOSED: Mod response channel closed".into()),
        Err(_) => {
            session.cancel(id, &full_id, &generation);
            Err("TIMEOUT: Mod did not reply within 5 seconds".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_validation_requires_bounded_ascii() {
        assert!(super::super::valid_rpc_name("hello-world"));
        assert!(super::super::valid_rpc_name("status.read"));
        assert!(!super::super::valid_rpc_name("../dll"));
        assert!(!super::super::valid_rpc_name(""));
    }
}
