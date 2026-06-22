use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "bmcbl-plugin-tools")]
#[command(about = "Build and package BMCBL WASM plugins")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Pack {
        #[arg(long)]
        manifest_path: PathBuf,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Pack {
            manifest_path,
            release,
            out_dir,
        } => {
            let result =
                bmcbl_plugin_api::pack::pack_plugin(bmcbl_plugin_api::pack::PackOptions {
                    manifest_path,
                    release,
                    out_dir,
                    run_cargo_build: true,
                    target_dir: None,
                    wasm_path: None,
                })?;
            println!("{}", result.package_path.display());
        }
    }

    Ok(())
}
