use bmcbl_plugin_api::prelude::*;

const PLUGIN: PluginMetadata = plugin_metadata!();

struct EssentialsPlugin;

plugin_actions! {
    pub enum EssentialsAction {
        OpenWindow = "open-window",
    }
}

#[bmcbl_plugin]
impl Plugin for EssentialsPlugin {
    fn init(context: PluginContext) -> PluginResult<Vec<Registration>> {
        log_info!("{} initialized for {}", PLUGIN.name, context.plugin_id);
        Ok(registrations! {
            page "main", title = PLUGIN.name, nav = Nav::new("Essentials").icon("plug").order(1000);
            injection InjectionSlot::MainRootOverlay, priority = 1000;
            subscribe "route-changed";
        })
    }

    fn handle_event(event: HostEvent) -> PluginResult<()> {
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
