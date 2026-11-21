//! Authenticated Linux host transport for a Windows BLoader DLL under Wine,
//! Proton or UMU. The DLL remains in the shared BMCBL/plugins directory.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

use super::{LaunchPlan, new_session_id, serve};

const MAGIC: &[u8; 8] = b"BVMOD1\0\0";
const DOMAIN: &[u8] = b"BMCBL-VMOD-IPC-V1";
const BOOTSTRAP_ENV: &str = "BMCBL_VMOD_BOOTSTRAP";

/// Stored only in a one-off 0700 directory; the path, never the secret, is
/// passed through Proton/UMU's launch environment to the Windows guest.
pub(crate) struct PendingLinuxSession {
    bootstrap_dir: TempDir,
    bootstrap_path_in_wine: String,
    listener: TcpListener,
    key: [u8; 32],
    session_id: String,
    plan: LaunchPlan,
    plugins_root_in_wine: String,
    game_version: String,
}

pub(crate) fn prepare(
    plan: LaunchPlan,
    game_version: String,
    wine_prefix: &Path,
) -> Result<PendingLinuxSession, String> {
    let wine_root = wine_visible_path(&plan.plugins_root, wine_prefix)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("bind virtual Mod loopback endpoint: {error}"))?;
    listener.set_nonblocking(true)
        .map_err(|error| format!("configure virtual Mod endpoint: {error}"))?;
    let address = listener.local_addr()
        .map_err(|error| format!("virtual Mod endpoint address unavailable: {error}"))?;
    let key: [u8; 32] = rand::random();
    let session_id = new_session_id();
    // This ephemeral directory contains authentication metadata only. It
    // never contains a Mod DLL and does not add a second Mod installation.
    let bootstrap_dir = Builder::new().prefix("bmcbl-virtual-mod-ipc-").tempdir()
        .map_err(|error| format!("create private virtual Mod IPC descriptor: {error}"))?;
    fs::set_permissions(bootstrap_dir.path(), fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect private virtual Mod session: {error}"))?;
    let descriptor_path = bootstrap_dir.path().join("bootstrap.json");
    let bootstrap_path_in_wine = wine_visible_path(&descriptor_path, wine_prefix)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?.as_secs();
    let descriptor = json!({
        "session_id": session_id,
        "transport": "authenticated-loopback",
        "endpoint": address.to_string(),
        "session_secret": STANDARD.encode(key),
        "expires_at": now.saturating_add(30)
    });
    let serialized = serde_json::to_vec(&descriptor)
        .map_err(|error| format!("serialize virtual Mod descriptor: {error}"))?;
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(&descriptor_path)
        .map_err(|error| format!("write private virtual Mod descriptor: {error}"))?;
    file.write_all(&serialized).map_err(|error| format!("save Mod descriptor: {error}"))?;
    file.sync_all().map_err(|error| format!("flush Mod descriptor: {error}"))?;
    Ok(PendingLinuxSession {
        bootstrap_dir, bootstrap_path_in_wine, listener, key, session_id,
        plan, plugins_root_in_wine: wine_root, game_version,
    })
}

impl PendingLinuxSession {
    pub(crate) fn apply_to_command(&self, command: &mut tokio::process::Command) {
        command.env(BOOTSTRAP_ENV, &self.bootstrap_path_in_wine);
    }

    /// The worker must start before spawning the Proton/UMU game process.
    /// The game is not blocked if no authenticated BLoader joins in two seconds.
    pub(crate) fn start(self) -> Result<(), String> {
        thread::Builder::new().name("bmcbl-virtual-mod-wine".to_owned())
            .spawn(move || {
                if let Err(error) = self.run() {
                    tracing::warn!(%error, "Linux virtual Mod session skipped or closed");
                }
            })
            .map(|_| ())
            .map_err(|error| format!("cannot start Linux Mod session: {error}"))
    }

    fn run(self) -> Result<(), String> {
        let start = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        return Err("untrusted virtual Mod peer is not loopback".into());
                    }
                    stream.set_read_timeout(Some(Duration::from_secs(2)))
                        .map_err(|error| error.to_string())?;
                    stream.set_write_timeout(Some(Duration::from_secs(2)))
                        .map_err(|error| error.to_string())?;
                    authenticate(&mut stream, &self.session_id, &self.key)?;
                    // The descriptor must survive until the guest reads it;
                    // after authentication, remove it immediately.
                    let descriptor_path = self.bootstrap_dir.path().join("bootstrap.json");
                    let _ = fs::remove_file(&descriptor_path);
                    stream.set_read_timeout(None).map_err(|error| error.to_string())?;
                    stream.set_write_timeout(None).map_err(|error| error.to_string())?;
                    return serve(
                        &mut stream, &self.plan, &self.plugins_root_in_wine,
                        "authenticated-loopback", None, &self.session_id, &self.game_version,
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                    && start.elapsed() < Duration::from_secs(2) =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => {
                    return Err(format!("Wine virtual Mod connection timed out: {error}"));
                }
            }
        }
    }
}

fn wine_visible_path(path: &Path, prefix: &Path) -> Result<String, String> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| format!("canonicalize Wine-visible path: {error}"))?;
    if !resolved.is_absolute() {
        return Err("virtual Mod path is not absolute".into());
    }
    // Only permit Z: when the active prefix explicitly maps it to /.
    // A missing Z: mapping must fail closed, not send a guessed Windows path.
    let z_drive = prefix.join("dosdevices").join("z:");
    let mapped = fs::canonicalize(&z_drive)
        .map_err(|_| "Wine prefix has no verified Z: mapping; virtual Mods disabled".to_owned())?;
    if mapped != Path::new("/") {
        return Err("Wine prefix Z: is not mapped to Linux /; virtual Mods disabled".into());
    }
    let path = resolved.to_str()
        .ok_or("virtual Mod path is not valid UTF-8 for Wine")?;
    if path.contains('\0') || path.contains('\r') || path.contains('\n') {
        return Err("virtual Mod path contains forbidden control characters".into());
    }
    Ok(format!("Z:{}", path.replace('/', "\\")))
}

/// Exact counterpart of BLoader's pre-protocol v1 binary mutual challenge.
fn authenticate(stream: &mut TcpStream, session: &str, key: &[u8; 32]) -> Result<(), String> {
    let mut magic = [0_u8; 8];
    stream.read_exact(&mut magic).map_err(|_| "Wine Mod client challenge unavailable")?;
    if &magic != MAGIC {
        return Err("virtual Mod authentication magic mismatch".into());
    }
    let mut client_nonce = [0_u8; 32];
    stream.read_exact(&mut client_nonce).map_err(|_| "Wine Mod client nonce unavailable")?;
    let host_nonce: [u8; 32] = rand::random();
    stream.write_all(MAGIC).map_err(|_| "Wine Mod server challenge unavailable")?;
    stream.write_all(&host_nonce).map_err(|_| "Wine Mod server nonce unavailable")?;
    let mut proof = [0_u8; 32];
    stream.read_exact(&mut proof).map_err(|_| "Wine Mod client proof unavailable")?;
    let expected = mac(key, session, &client_nonce, &host_nonce, b"loader");
    let mut difference = 0u8;
    for (left, right) in proof.iter().zip(expected.iter()) {
        difference |= left ^ right;
    }
    if difference != 0 {
        return Err("Wine Mod authentication proof mismatch".into());
    }
    let response = mac(key, session, &client_nonce, &host_nonce, b"host");
    stream.write_all(&response).map_err(|_| "Wine Mod host proof unavailable")?;
    Ok(())
}

fn mac(key: &[u8], session: &str, client: &[u8; 32], server: &[u8; 32], role: &[u8]) -> [u8; 32] {
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, value) in key.iter().enumerate() {
        inner_pad[index] ^= value;
        outer_pad[index] ^= value;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(DOMAIN);
    inner.update(session.as_bytes());
    inner.update(client);
    inner.update(server);
    inner.update(role);
    let digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(digest);
    outer.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_roles_are_distinct() {
        let key = [7u8; 32];
        let nonce = [11u8; 32];
        assert_ne!(mac(&key, "session", &nonce, &nonce, b"loader"),
                   mac(&key, "session", &nonce, &nonce, b"host"));
    }

    #[test]
    fn rejects_unmapped_wine_root() {
        assert!(wine_visible_path(Path::new("/"), Path::new("/missing-prefix")).is_err());
    }
}
