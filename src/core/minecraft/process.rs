use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

const DISCOVERY_PORT: u16 = 7551;
const TERMINATION_WAIT: Duration = Duration::from_secs(3);
const TERMINATION_POLL_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortOwnerTermination {
    pub matched: usize,
    pub terminated: usize,
}

pub async fn terminate_discovery_port_owners() -> Result<PortOwnerTermination, String> {
    crate::tasks::runtime::run_io_blocking(terminate_discovery_port_owners_blocking).await?
}

fn terminate_discovery_port_owners_blocking() -> Result<PortOwnerTermination, String> {
    let current_process_id = std::process::id();
    let process_ids = discovery_port_owner_pids(DISCOVERY_PORT)?
        .into_iter()
        .filter(|process_id| *process_id != current_process_id)
        .collect::<Vec<_>>();
    if process_ids.is_empty() {
        return Ok(PortOwnerTermination {
            matched: 0,
            terminated: 0,
        });
    }

    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut signal_failures = Vec::new();
    for process_id in &process_ids {
        let process_id = Pid::from_u32(*process_id);
        let Some(process) = system.process(process_id) else {
            continue;
        };
        if !process.kill() {
            signal_failures.push(format!(
                "{} (PID {process_id})",
                process.name().to_string_lossy()
            ));
        }
    }

    let deadline = Instant::now() + TERMINATION_WAIT;
    loop {
        system.refresh_processes(ProcessesToUpdate::All, true);
        let remaining = process_ids
            .iter()
            .filter(|process_id| system.process(Pid::from_u32(**process_id)).is_some())
            .count();
        if remaining == 0 {
            return Ok(PortOwnerTermination {
                matched: process_ids.len(),
                terminated: process_ids.len(),
            });
        }
        if Instant::now() >= deadline {
            let terminated = process_ids.len().saturating_sub(remaining);
            let detail = if signal_failures.is_empty() {
                format!("仍有 {remaining} 个占用进程未退出")
            } else {
                format!("无法结束：{}", signal_failures.join("、"))
            };
            return Err(format!(
                "已结束 {terminated} 个 UDP 7551 占用进程，但{detail}；请手动关闭后重新检查"
            ));
        }
        std::thread::sleep(TERMINATION_POLL_INTERVAL);
    }
}

#[cfg(target_os = "windows")]
fn discovery_port_owner_pids(port: u16) -> Result<Vec<u32>, String> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedUdpTable, MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, UDP_TABLE_OWNER_PID,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    fn query_rows<Row: Copy>(
        address_family: u32,
        row_port: impl Fn(&Row) -> u16,
        row_pid: impl Fn(&Row) -> u32,
        port: u16,
    ) -> Result<Vec<u32>, String> {
        const ERROR_INSUFFICIENT_BUFFER_CODE: u32 = 122;
        let mut byte_size = 0_u32;
        // SAFETY: the first call passes no output buffer and only asks Windows for its size.
        let size_result = unsafe {
            GetExtendedUdpTable(
                None,
                &mut byte_size,
                false,
                address_family,
                UDP_TABLE_OWNER_PID,
                0,
            )
        };
        if size_result != ERROR_INSUFFICIENT_BUFFER_CODE && size_result != 0 {
            return Err(format!(
                "查询 UDP {port} 占用进程所需缓冲区失败：Windows 错误 {size_result}"
            ));
        }

        let mut buffer = vec![0_u8; byte_size as usize];
        // SAFETY: `buffer` is writable for `byte_size` bytes and remains alive for the call.
        let query_result = unsafe {
            GetExtendedUdpTable(
                Some(buffer.as_mut_ptr().cast()),
                &mut byte_size,
                false,
                address_family,
                UDP_TABLE_OWNER_PID,
                0,
            )
        };
        if query_result != 0 {
            return Err(format!(
                "查询 UDP {port} 占用进程失败：Windows 错误 {query_result}"
            ));
        }
        if buffer.len() < std::mem::size_of::<u32>() {
            return Err("Windows 返回了无效的 UDP 端点表".to_string());
        }

        // SAFETY: the length check above guarantees the entry count is readable. UDP table rows
        // follow the count directly; `read_unaligned` avoids assuming the byte buffer alignment.
        let entry_count = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<u32>()) };
        let row_size = std::mem::size_of::<Row>();
        let rows_size = (entry_count as usize)
            .checked_mul(row_size)
            .and_then(|size| size.checked_add(std::mem::size_of::<u32>()))
            .ok_or_else(|| "Windows UDP 端点表大小溢出".to_string())?;
        if rows_size > buffer.len() {
            return Err("Windows 返回的 UDP 端点表长度不完整".to_string());
        }

        let mut process_ids = Vec::new();
        for index in 0..entry_count as usize {
            let row_offset = std::mem::size_of::<u32>() + index * row_size;
            // SAFETY: `rows_size` was validated against the buffer and the row is copied with an
            // unaligned read, so no borrowed reference outlives `buffer`.
            let row =
                unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(row_offset).cast::<Row>()) };
            if row_port(&row) == port {
                process_ids.push(row_pid(&row));
            }
        }
        Ok(process_ids)
    }

    let mut process_ids = BTreeSet::new();
    process_ids.extend(query_rows::<MIB_UDPROW_OWNER_PID>(
        AF_INET.0.into(),
        |row| u16::from_be(row.dwLocalPort as u16),
        |row| row.dwOwningPid,
        port,
    )?);
    process_ids.extend(query_rows::<MIB_UDP6ROW_OWNER_PID>(
        AF_INET6.0.into(),
        |row| u16::from_be(row.dwLocalPort as u16),
        |row| row.dwOwningPid,
        port,
    )?);
    Ok(process_ids.into_iter().collect())
}

#[cfg(target_os = "linux")]
fn discovery_port_owner_pids(port: u16) -> Result<Vec<u32>, String> {
    use std::fs;

    let mut socket_inodes = BTreeSet::new();
    for table_path in ["/proc/net/udp", "/proc/net/udp6"] {
        let table = fs::read_to_string(table_path)
            .map_err(|error| format!("读取 {table_path} 失败：{error}"))?;
        socket_inodes.extend(parse_linux_udp_inodes(&table, port));
    }
    if socket_inodes.is_empty() {
        return Ok(Vec::new());
    }

    let mut process_ids = BTreeSet::new();
    let process_entries =
        fs::read_dir("/proc").map_err(|error| format!("读取 /proc 失败：{error}"))?;
    for entry in process_entries.flatten() {
        let Some(process_id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(descriptors) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let owns_socket = descriptors.flatten().any(|descriptor| {
            fs::read_link(descriptor.path())
                .ok()
                .and_then(|target| target.to_str().map(str::to_owned))
                .and_then(|target| {
                    target
                        .strip_prefix("socket:[")
                        .and_then(|value| value.strip_suffix(']'))
                        .and_then(|value| value.parse::<u64>().ok())
                })
                .is_some_and(|inode| socket_inodes.contains(&inode))
        });
        if owns_socket {
            process_ids.insert(process_id);
        }
    }
    Ok(process_ids.into_iter().collect())
}

#[cfg(any(test, target_os = "linux"))]
fn parse_linux_udp_inodes(table: &str, port: u16) -> BTreeSet<u64> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
            let local_address = fields.get(1)?;
            let port_hex = local_address.rsplit_once(':')?.1;
            (u16::from_str_radix(port_hex, 16).ok()? == port)
                .then(|| fields.get(9)?.parse::<u64>().ok())
                .flatten()
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn discovery_port_owner_pids(port: u16) -> Result<Vec<u32>, String> {
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", &format!("-iUDP:{port}"), "-Fpn"])
        .output()
        .map_err(|error| format!("启动 macOS UDP 端口查询失败：{error}"))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(format!(
            "macOS UDP 端口查询失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_lsof_udp_owner_pids(
        &String::from_utf8_lossy(&output.stdout),
        port,
    ))
}

#[cfg(any(test, target_os = "macos"))]
fn parse_lsof_udp_owner_pids(output: &str, port: u16) -> Vec<u32> {
    let mut current_process_id = None;
    let mut process_ids = BTreeSet::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            current_process_id = value.parse::<u32>().ok();
            continue;
        }
        let Some(address) = line.strip_prefix('n') else {
            continue;
        };
        let local_address = address.split_once("->").map_or(address, |(local, _)| local);
        let local_port = local_address
            .rsplit_once(':')
            .and_then(|(_, value)| value.parse::<u16>().ok());
        if local_port == Some(port)
            && let Some(process_id) = current_process_id
        {
            process_ids.insert(process_id);
        }
    }
    process_ids.into_iter().collect()
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn discovery_port_owner_pids(_port: u16) -> Result<Vec<u32>, String> {
    Err("当前平台暂不支持查询 UDP 7551 占用进程".to_string())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    #[test]
    fn finds_current_process_that_owns_udp_port() {
        let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).expect("应绑定测试 UDP 端口");
        let port = socket.local_addr().expect("应读取测试端口").port();

        let process_ids = super::discovery_port_owner_pids(port).expect("应查询 UDP 占用进程");

        assert!(
            process_ids.contains(&std::process::id()),
            "UDP 端点表未返回当前测试进程：{process_ids:?}"
        );
    }

    #[test]
    fn parses_linux_udp_socket_inode_for_requested_port() {
        let table = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
  12: 00000000:1D7F 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 99881
  13: 00000000:4ABC 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 99882
";

        assert_eq!(
            super::parse_linux_udp_inodes(table, 7551)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![99881]
        );
    }

    #[test]
    fn parses_only_lsof_processes_with_matching_local_udp_port() {
        let output = "\
p100
f12
n*:7551
p200
f8
n127.0.0.1:50000->192.0.2.1:7551
p300
f9
n[::]:7551
";

        assert_eq!(
            super::parse_lsof_udp_owner_pids(output, 7551),
            vec![100, 300]
        );
    }
}
