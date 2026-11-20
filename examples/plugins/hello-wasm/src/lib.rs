use bmcbl_plugin_api::prelude::*;
use bmcbl_plugin_api::{r#async::Job, world};
use std::cell::RefCell;

const PLUGIN: PluginMetadata = plugin_metadata!();

struct EssentialsPlugin;

thread_local! {
    static SLIME_QUERY: RefCell<Option<Job<bool>>> = const { RefCell::new(None) };
}

plugin_actions! {
    pub enum EssentialsAction {
        OpenWindow = "open-window",
    }
}

#[bmcbl_plugin]
impl Plugin for EssentialsPlugin {
    fn init(context: PluginContext) -> PluginResult<Vec<Registration>> {
        log_info!("{} initialized for {}", PLUGIN.name, context.plugin_id);
        if let Ok(job) = world::is_slime_chunk(-2, 7) {
            SLIME_QUERY.set(Some(job));
        }
        Ok(registrations! {
            page "main", title = PLUGIN.name, tool = Tool::new("Essentials").icon("plug").order(1000);
            injection InjectionSlot::MainRootOverlay, priority = 1000;
            subscribe "route-changed";
        })
    }

    fn handle_event(event: PluginEvent) -> PluginResult<()> {
        SLIME_QUERY.with_borrow_mut(|pending| {
            if pending.as_ref().is_some_and(|job| job.is_ready(&event))
                && let Some(job) = pending.take()
                && let Some(is_slime_chunk) = job.try_take()?
            {
                log_info!("host world query: {is_slime_chunk}");
            }
            Ok::<(), PluginError>(())
        })?;
        if event.action_is(EssentialsAction::OpenWindow.as_str()) {
            log_info!("open-window action received");
            toast!(success, tr!("essentials.toast.opened"))?;
            PLUGIN
                .window("main")
                .title(PLUGIN.name)
                .size(780, 520)
                .resizable(true)
                .open(PLUGIN)?;
            invalidate!(page "main")?;
        }

        if let Some(path) = event.route_path() {
            log_debug!("route changed: {path}");
        }

        Ok(())
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree> {
        let config_bytes = read_config().map_or(0, |config| config.len());
        Ok(view! {
            column(padding = 24, gap = 12) {
                badge(tr!("essentials.badge"));
                title(tr!("essentials.title"));
                text(tr!("essentials.summary"));
                text(tr!("essentials.plugin_id", "id" => PLUGIN.id));
                text(tr!("essentials.version", "version" => PLUGIN.version));
                text(tr!("essentials.authors", "authors" => PLUGIN.authors_display()));
                text(tr!("essentials.capabilities", "capabilities" => PLUGIN.capabilities_display()));
                text(tr!("essentials.config_status", "bytes" => config_bytes));
                button(
                    tr!("essentials.open_window", "page" => request.page_id),
                    EssentialsAction::OpenWindow.as_str(),
                );
            }
        })
    }

    fn render_injection(_request: InjectionRequest) -> PluginResult<Option<ViewTree>> {
        Ok(Some(view! {
            badge(tr!("essentials.overlay"))
        }))
    }

    fn shutdown(reason: ShutdownReason) -> PluginResult<()> {
        log_info!("{} shutdown: {:?}", PLUGIN.name, reason);
        Ok(())
    }
}
