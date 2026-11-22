#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    match bmcbl::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            bmcbl::print_cli_error(&format!("Error: {error:#}\n"));
            std::process::ExitCode::from(1)
        }
    }
}
