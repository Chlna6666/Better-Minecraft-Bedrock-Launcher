#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    refuse_root_gui()?;

    bmcbl::run()
}

#[cfg(target_os = "linux")]
fn refuse_root_gui() -> anyhow::Result<()> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let effective_user_id = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|uids| uids.split_whitespace().nth(1))
        .and_then(|uid| uid.parse::<u32>().ok());

    if effective_user_id == Some(0) {
        anyhow::bail!(
            "请不要以 root 身份运行 BMCBL。请使用普通桌面用户启动；需要安装系统依赖时，BMCBL 将单独请求授权。"
        );
    }

    Ok(())
}
