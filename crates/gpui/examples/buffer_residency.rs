//! Native Buffer placement baseline; never changes GPUI's production policy.
#![cfg_attr(
    any(windows, all(feature = "nova-gfx-opengl", target_os = "linux")),
    expect(
        unsafe_code,
        reason = "the native lab owns its GL bootstrap and synchronous FXC compiler FFI"
    )
)]
mod buffer_residency_lab;

fn main() -> anyhow::Result<()> {
    let options = buffer_residency_lab::Options::parse(std::env::args().skip(1))?;
    let report = buffer_residency_lab::run_backend(&options)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
