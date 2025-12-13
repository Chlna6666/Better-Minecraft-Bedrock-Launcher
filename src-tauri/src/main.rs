// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process;
use clap::{Parser, Subcommand};
use tracing::{error, info};

// 导入我们封装好的 unpack_gdk 函数
use app_lib::core::minecraft::gdk::unpack_gdk;
use app_lib::utils::logger::init_logging;
use app_lib::{run, show_windows_error};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 解包一个 GDK (MSIX-VC) 文件
    GdkUnpack {
        /// 需要解包的输入文件路径
        #[arg(short, long)]
        input: PathBuf,

        /// 解包后文件存放的输出目录
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_logging();

    let cli = Cli::parse();

    // 检查是否有关 GDK 解包的命令行指令
    if let Some(Commands::GdkUnpack { input, output }) = cli.command {
        info!("检测到 'gdk-unpack' 命令，开始执行解包...");

        // 调用封装好的解包函数
        unpack_gdk(input, output);

        info!("解包任务执行完毕，程序退出。");
        process::exit(0);
    }

    // 如果没有 gdk-unpack 命令，则执行原来的 GUI 程序逻辑
    info!("未检测到特定子命令，启动主程序 GUI...");

    // --- 原来的 main 函数逻辑 ---
    // ... (为了简洁，这里省略了原有的代码，但它们应该保持不变)
    // 读取配置、初始化 i18n、检查 WebView2 等
    match app_lib::config::config::read_config() {
        Ok(config) => {
            let preinit = std::sync::Arc::new(app_lib::PreInit {
                config,
                locale: "en-US".to_string(), // 简化示例
                webview2_ver: "unknown".to_string(), // 简化示例
            });
            if let Err(e) = run(preinit).await {
                let err_msg = format!("程序运行失败: {:?}", e);
                error!("{}", err_msg);
                show_windows_error("程序运行失败", &err_msg);
                process::exit(1);
            }
        }
        Err(e) => {
            let msg = format!("读取配置失败: {:?}\n程序将退出。", e);
            error!("{}", msg);
            show_windows_error("启动失败 - 读取配置", &msg);
            process::exit(1);
        }
    }
}
