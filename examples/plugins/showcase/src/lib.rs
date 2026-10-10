//! BMCBL 插件功能全集示例。
//!
//! 一个插件覆盖插件 API 的主要能力：
//!
//! - 注册与导航：主页面、独立窗口页面、模态框页面、两处 UI 注入；
//! - 交互：按钮、带值按钮、复选框、开关、下拉框、进度条、链接、图标、图片、容器与卡片；
//! - 事件：`route-changed` 订阅与插件自定义 `emit_event` 广播；
//! - 状态：session 会话值 + storage 持久化键值；
//! - 宿主能力：配置读写、本地化、HTTP 文本缓存、资源读取、剪贴板、外链、导航、任务进度；
//! - 生命周期：`init` / `handle_event` / `render_page` / `render_injection` / `shutdown`。
//!
//! 代码组织：`state` 维护会话与持久状态，`actions` 分发动作，`views` 只负责渲染。
//!
//! 构建与打包（需要 `wasm32-unknown-unknown` 目标）：
//!
//! ```text
//! cargo build --release --target wasm32-unknown-unknown \
//!     --manifest-path examples/plugins/showcase/Cargo.toml
//! ```
//!
//! 打包脚本会从 `[package.metadata.bmcbl-plugin]` 生成 `plugin.toml`，并把 `.bmcblx` 包与
//! 解包目录写到 `target/bmcbl-plugin-auto-build/bmcbl-showcase/` 下，直接放进 BMCBL 的
//! `plugins/` 目录即可加载。

mod actions;
mod state;
mod views;

use bmcbl_plugin_api::prelude::*;

pub use actions::ShowcaseAction;

/// 插件元数据，来自 `Cargo.toml` 的 `[package.metadata.bmcbl-plugin]`。
pub const PLUGIN: PluginMetadata = plugin_metadata!();

/// 主页面 id（注册了导航入口）。
pub const PAGE_MAIN: &str = "main";
/// 独立窗口页面 id。
pub const PAGE_WINDOW: &str = "window";
/// 模态框页面 id。
pub const PAGE_MODAL: &str = "modal";

/// 插件自己广播的全局事件名，本插件也订阅它。
pub const EVENT_PING: &str = "showcase:ping";

/// 示例资源路径，与 `Cargo.toml` 的 `permissions.resource` 保持一致。
pub const RESOURCE_NOTICE: &str = "assets/notice.txt";

struct ShowcasePlugin;

#[bmcbl_plugin]
impl Plugin for ShowcasePlugin {
    fn init(context: PluginContext) -> PluginResult<Vec<Registration>> {
        // 恢复持久状态，并把默认值写进 session，供渲染阶段直接读取。
        state::restore()?;
        log_info!("{} initialized for {}", PLUGIN.name, context.plugin_id);

        Ok(registrations! {
            page PAGE_MAIN, title = PLUGIN.name, nav = Nav::new(tr!("showcase.nav")).icon("plug").order(40);
            page PAGE_WINDOW, title = tr!("showcase.window.title");
            page PAGE_MODAL, title = tr!("showcase.modal.title");
            injection InjectionSlot::MainRootOverlay, page = PAGE_MAIN, priority = 20, layout = InjectionLayout::sidebar();
            injection InjectionSlot::HomeSidebar, page = "/", priority = 30, layout = InjectionLayout::sidebar().width(300).compact_behavior(CompactBehavior::Scroll);
            subscribe "route-changed";
            subscribe EVENT_PING;
        })
    }

    fn handle_event(event: HostEvent) -> PluginResult<()> {
        actions::handle(event)
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree> {
        Ok(views::page(&request.page_id))
    }

    fn render_injection(request: InjectionRequest) -> PluginResult<Option<ViewTree>> {
        Ok(views::injection(request.slot))
    }

    fn shutdown(reason: ShutdownReason) -> PluginResult<()> {
        // 卸载/重载前把会话值落盘，插件重载后由 `init` 读回。
        state::persist()?;
        log_info!("{} shutdown: {:?}", PLUGIN.name, reason);
        Ok(())
    }
}
