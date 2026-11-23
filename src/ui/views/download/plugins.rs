use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;

use crate::downloads::verified;
use crate::plugins::{registry, runtime};
use crate::tasks::task_manager::wait_for_task;
use crate::ui::components::button::{ghost_button, primary_button, secondary_button};
use crate::ui::components::markdown::{Document, parse_document, render_document};
use crate::ui::components::page_shell::glass_card;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::toast;
use crate::ui::components::{dialog, modal};
use crate::ui::components::{plugin_icon, plugin_tags};
use crate::ui::state::i18n::I18n;
use crate::ui::theme::ThemeColors;

use super::state::{DownloadPageState, DownloadTab};

const PAGE_SIZE: usize = 20;

#[derive(Default)]
pub(super) struct State {
    catalog: Option<registry::Catalog>,
    pub(super) loading: bool,
    error: Option<String>,
    installing: BTreeSet<String>,
    details: Option<Details>,
}

struct Details {
    plugin: registry::Plugin,
    readme: Readme,
    load: Option<tokio::task::AbortHandle>,
}

impl DownloadPageState {
    pub(crate) fn plugin_details_open(&self) -> bool {
        self.tab == DownloadTab::Plugin && self.plugin_catalog.details.is_some()
    }

    pub(crate) fn hash_plugin_details(&self, hasher: &mut impl Hasher) {
        if let Some(details) = &self.plugin_catalog.details {
            details.plugin.id.hash(hasher);
            details.plugin.version.hash(hasher);
            self.plugin_catalog
                .installing
                .contains(&details.plugin.id)
                .hash(hasher);
            std::mem::discriminant(&details.readme).hash(hasher);
            details.load.as_ref().map(|load| load.id()).hash(hasher);
            if let Readme::Error(error) = &details.readme {
                error.hash(hasher);
            }
        }
    }
}

impl State {
    pub(super) fn dismiss_details(&mut self) {
        self.details = None;
    }
}

enum Readme {
    Loading,
    Ready(Option<Document>),
    Error(String),
}

impl Drop for Details {
    fn drop(&mut self) {
        if let Some(load) = &self.load {
            load.abort();
        }
    }
}

pub(crate) fn load(cx: &mut App, refresh: bool) {
    let previous = cx.update_global(|state: &mut DownloadPageState, _| {
        let state = &mut state.plugin_catalog;
        if state.loading || (!refresh && (state.catalog.is_some() || state.error.is_some())) {
            return None;
        }
        state.loading = true;
        state.error = None;
        Some(state.catalog.clone().unwrap_or_default())
    });
    let Some(previous) = previous else { return };
    cx.spawn(async move |cx| {
        let result = match crate::tasks::runtime::spawn_io(registry::catalog(previous)) {
            Ok(job) => job
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Err(error) => Err(error),
        };
        let _ = cx.update_global(|state: &mut DownloadPageState, _| {
            state.plugin_catalog.loading = false;
            match result {
                Ok(catalog) => state.plugin_catalog.catalog = Some(catalog),
                Err(error) => state.plugin_catalog.error = Some(error),
            }
        });
    })
    .detach();
}

pub(super) fn render(
    colors: &ThemeColors,
    cx: &App,
    image_cache: &Entity<BoundedImageCache>,
) -> Div {
    let state = cx.global::<DownloadPageState>();
    let registry = &state.plugin_catalog;
    let query = state.search_query.trim().to_lowercase();
    let entries: Vec<_> = registry
        .catalog
        .iter()
        .flat_map(|catalog| catalog.plugins.iter())
        .filter(|plugin| {
            query.is_empty()
                || [
                    &plugin.name,
                    &plugin.id,
                    &plugin.publisher,
                    &plugin.description,
                ]
                .into_iter()
                .any(|value| value.to_lowercase().contains(&query))
        })
        .collect();
    let pages = entries.len().div_ceil(PAGE_SIZE).max(1);
    let page = state.page_index.min(pages - 1);
    let mut list = div()
        .id(("plugin-catalog", page))
        .overflow_y_scrollbar()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.));
    if let Some(error) = &registry.error {
        list = list.child(div().text_color(colors.danger).child(error.clone()));
    }
    if registry.loading {
        list = list.child(
            div()
                .text_color(colors.text_secondary)
                .child(t!("DownloadPage.plugins_loading")),
        );
    } else if entries.is_empty() && registry.error.is_none() {
        list = list.child(
            div()
                .text_color(colors.text_secondary)
                .child(t!("DownloadPage.plugins_empty")),
        );
    }
    let first = page * PAGE_SIZE;
    list = list.children(entries[first..].iter().take(PAGE_SIZE).map(|plugin| {
        catalog_row(
            colors,
            cx,
            image_cache,
            plugin,
            registry.installing.contains(&plugin.id),
        )
    }));
    let mut panel = div()
        .size_full()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(list);
    if pages > 1 {
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .p(px(10.))
                .child(
                    secondary_button(colors, "plugins-prev", "‹")
                        .when(page == 0, |button| button.opacity(0.55).cursor_default())
                        .when(page > 0, |button| {
                            button.on_click(move |_, _, cx| set_page(cx, page - 1))
                        }),
                )
                .child(format!("{} / {pages}", page + 1))
                .child(
                    secondary_button(colors, "plugins-next", "›")
                        .when(page + 1 >= pages, |button| {
                            button.opacity(0.55).cursor_default()
                        })
                        .when(page + 1 < pages, |button| {
                            button.on_click(move |_, _, cx| set_page(cx, page + 1))
                        }),
                ),
        );
    }
    panel
}

fn catalog_row(
    colors: &ThemeColors,
    cx: &App,
    image_cache: &Entity<BoundedImageCache>,
    plugin: &registry::Plugin,
    busy: bool,
) -> Stateful<Div> {
    let entry = plugin.clone();
    let hover = colors.surface_hover;
    glass_card(colors)
        .shadow(Vec::new())
        .id(SharedString::from(format!("catalog-row-{}", plugin.id)))
        .debug_selector(|| format!("catalog-row-{}", plugin.id))
        .w_full()
        .flex_none()
        .px(px(12.))
        .py(px(8.))
        .line_height(px(16.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| {
            window.blur(cx);
            open_details(cx, entry.clone());
        })
        .child(icon(colors, image_cache, plugin))
        .child(catalog_summary(colors, plugin))
        .child(
            div()
                .flex_none()
                .max_w(relative(0.36))
                .flex()
                .flex_col()
                .items_end()
                .gap(px(2.))
                .child(
                    install_button(colors, cx, plugin, busy)
                        .h(px(30.))
                        .px(px(11.))
                        .py(px(0.)),
                )
                .children(
                    github_downloads(colors, plugin).map(|count| count.max_w_full().truncate()),
                ),
        )
}

fn catalog_summary(colors: &ThemeColors, plugin: &registry::Plugin) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(14.))
                .line_height(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(plugin.name.clone()),
        )
        .when(!plugin.description.trim().is_empty(), |item| {
            item.child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .truncate()
                    .child(super::common::sanitize_single_line(&plugin.description)),
            )
        })
        .child(catalog_metadata(colors, plugin))
}

fn catalog_metadata(colors: &ThemeColors, plugin: &registry::Plugin) -> Div {
    let tags = plugin_tags::render(colors, &plugin.tags[..plugin.tags.len().min(2)]);
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .min_w(px(0.))
                .text_size(px(11.))
                .line_height(px(14.))
                .text_color(colors.text_secondary)
                .truncate()
                .child(plugin_metadata(plugin)),
        )
        .children(tags.map(|tags| tags.flex_1().flex_nowrap().overflow_hidden()))
}

fn github_downloads(colors: &ThemeColors, plugin: &registry::Plugin) -> Option<Div> {
    // The registry reports downloads summed over every release, so this is a
    // popularity figure rather than one version's count. Large numbers are
    // abbreviated: "13.4K" reads at a glance where "13421" does not.
    let count = crate::ui::views::download::common::format_count(
        plugin.github_download_count? as f64,
    );
    Some(
        div()
            .text_size(px(11.))
            .text_color(colors.text_secondary)
            .whitespace_nowrap()
            .child(t!("DownloadPage.plugins_github_downloads", count = count)),
    )
}

fn icon(
    colors: &ThemeColors,
    image_cache: &Entity<BoundedImageCache>,
    plugin: &registry::Plugin,
) -> AnyElement {
    let icon = plugin
        .icon
        .as_ref()
        .and_then(|icon| icon.url(&plugin.id))
        .map(|url| img(url).image_cache(image_cache));
    plugin_icon::render(colors, px(34.), icon)
}

fn plugin_metadata(plugin: &registry::Plugin) -> String {
    let status = match plugin.status.as_str() {
        "official" => t!("DownloadPage.plugins_official"),
        "verified" => t!("DownloadPage.plugins_verified"),
        _ => t!("DownloadPage.plugins_community"),
    };
    format!("{} · {status} · v{}", plugin.publisher, plugin.version)
}

fn summary(colors: &ThemeColors, plugin: &registry::Plugin) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(plugin.name.clone()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(plugin_metadata(plugin)),
        )
        .when(!plugin.description.trim().is_empty(), |item| {
            item.child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child(plugin.description.clone()),
            )
        })
        .children(plugin_tags::render(colors, &plugin.tags))
        .children(github_downloads(colors, plugin))
}

fn install_button(
    colors: &ThemeColors,
    cx: &App,
    plugin: &registry::Plugin,
    busy: bool,
) -> Stateful<Div> {
    let installed =
        runtime::plugin_manifest_snapshot(cx, &plugin.id).map(|manifest| manifest.version);
    let compatible = crate::plugins::manifest::validate_plugin_id(&plugin.id).is_ok();
    let current = installed.as_ref().is_some_and(|version| {
        version == &plugin.version
            || semver::Version::parse(version)
                .ok()
                .zip(semver::Version::parse(&plugin.version).ok())
                .is_some_and(|(installed, available)| installed >= available)
    });
    let action = if !compatible {
        t!("DownloadPage.plugins_incompatible")
    } else if busy {
        t!("DownloadPage.plugins_installing")
    } else if current {
        t!("DownloadPage.plugins_installed")
    } else if installed.is_some() {
        t!("DownloadPage.plugins_update")
    } else {
        t!("DownloadPage.plugins_install")
    };
    let enabled = compatible && !busy && !current;
    let plugin = plugin.clone();
    primary_button(
        colors,
        SharedString::from(format!("registry-{}", plugin.id)),
        action,
    )
    .flex_none()
    .when(!enabled, |button| button.opacity(0.55).cursor_default())
    .when(enabled, |button| {
        button.on_click(move |_, _, cx| {
            cx.stop_propagation();
            install(cx, plugin.clone());
        })
    })
}

pub(super) fn render_details(
    colors: &ThemeColors,
    window: &Window,
    cx: &App,
) -> Option<AnyElement> {
    let state = cx.global::<DownloadPageState>();
    if state.tab != DownloadTab::Plugin {
        return None;
    }
    let details = state.plugin_catalog.details.as_ref()?;
    let plugin = &details.plugin;
    let busy = state.plugin_catalog.installing.contains(&plugin.id);
    let image = plugin
        .icon
        .as_ref()
        .and_then(|icon| icon.url(&plugin.id))
        .map(img);
    let header = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(24.))
        .py(px(18.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .child(plugin_icon::render(colors, px(48.), image))
        .child(summary(colors, plugin).text_size(px(19.)));
    let readme = match &details.readme {
        Readme::Loading => div()
            .text_color(colors.text_secondary)
            .child(t!("common.loading")),
        Readme::Ready(Some(document)) => render_document(document, colors, colors.bg.l < 0.5),
        Readme::Ready(None) => div()
            .text_color(colors.text_secondary)
            .child(t!("PluginSettings.readme_empty")),
        Readme::Error(error) => {
            let plugin = plugin.clone();
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(10.))
                .child(div().text_color(colors.danger).child(error.clone()))
                .child(
                    secondary_button(colors, "plugin-readme-retry", t!("common.retry"))
                        .on_click(move |_, _, cx| open_details(cx, plugin.clone())),
                )
        }
    };
    let height = (window.bounds().size.height - px(64.))
        .min(px(680.))
        .max(px(0.));
    let width = (window.bounds().size.width - px(32.))
        .min(px(740.))
        .max(px(0.));
    let content = dialog::container(colors, width)
        .debug_selector(|| "plugin-readme-dialog".to_string())
        .h(height)
        .shadow_lg()
        .child(header)
        .child(
            div()
                .id(SharedString::from(format!(
                    "plugin-readme-{}-{}",
                    plugin.id, plugin.version
                )))
                .overflow_y_scrollbar()
                .flex_1()
                .min_h(px(0.))
                .px(px(24.))
                .py(px(18.))
                .child(readme),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_end()
                .gap(px(10.))
                .px(px(24.))
                .py(px(12.))
                .border_t_1()
                .border_color(Hsla {
                    a: 0.24,
                    ..colors.border
                })
                .child(
                    ghost_button(colors, "plugin-details-close", t!("common.close"))
                        .debug_selector(|| "plugin-readme-close".to_string())
                        .on_click(|_, _, cx| {
                            close_details(cx);
                        }),
                )
                .child(install_button(colors, cx, plugin, busy)),
        );
    Some(modal::dismissible_layer(
        content,
        hsla(0.0, 0.0, 0.0, 0.56),
        Rc::new(|cx| {
            close_details(cx);
        }),
        modal::DismissOptions::default(),
    ))
}

fn open_details(cx: &mut App, plugin: registry::Plugin) {
    let locale = cx.global::<I18n>().locale().code().to_owned();
    let id = plugin.id.clone();
    let version = plugin.version.clone();
    let job = crate::tasks::runtime::spawn_io(async move {
        registry::readme(id, version, locale).await.map(|readme| {
            readme
                .filter(|text| !text.trim().is_empty())
                .map(|text| parse_document(&text))
        })
    });
    let (readme, load) = match &job {
        Ok(job) => (Readme::Loading, Some(job.abort_handle())),
        Err(error) => (Readme::Error(error.clone()), None),
    };
    cx.update_global(|state: &mut DownloadPageState, _| {
        state.plugin_catalog.details = Some(Details {
            plugin,
            readme,
            load,
        });
    });
    let Ok(job) = job else { return };
    let id = job.id();
    cx.spawn(async move |cx| {
        let readme = match job
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        {
            Ok(document) => Readme::Ready(document),
            Err(error) => Readme::Error(error),
        };
        let _ = cx.update_global(|state: &mut DownloadPageState, _| {
            if let Some(details) = state.plugin_catalog.details.as_mut()
                && details.load.as_ref().is_some_and(|load| load.id() == id)
            {
                details.readme = readme;
                details.load = None;
            }
        });
    })
    .detach();
}

pub(crate) fn close_details(cx: &mut App) -> bool {
    let state = cx.global::<DownloadPageState>();
    if !state.plugin_details_open() {
        return false;
    }
    cx.update_global(|state: &mut DownloadPageState, _| state.plugin_catalog.dismiss_details());
    true
}

fn set_page(cx: &mut App, page: usize) {
    cx.update_global(|state: &mut DownloadPageState, _| {
        if state.tab == DownloadTab::Plugin {
            state.page_index = page;
        }
    });
}

fn install(cx: &mut App, plugin: registry::Plugin) {
    if !cx.update_global(|state: &mut DownloadPageState, _| {
        state.plugin_catalog.installing.insert(plugin.id.clone())
    }) {
        return;
    }
    cx.spawn(async move |cx| {
        let result = async {
            let id = plugin.id.clone();
            let version = plugin.version.clone();
            let package = crate::tasks::runtime::spawn_io(async move {
                let request = registry::package(&id, &version).await?;
                let task = verified::start(request)?;
                let result = completed(&task).await?;
                Ok::<_, String>(PathBuf::from(result))
            })?
            .await
            .map_err(|error| error.to_string())??;
            let task = cx
                .update(|cx| runtime::start_import_plugin_task(cx, package))
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            crate::tasks::runtime::spawn_io(async move { completed(&task).await })?
                .await
                .map_err(|error| error.to_string())??;
            Ok::<_, String>(())
        }
        .await;
        let _ = cx.update(|cx| {
            cx.update_global(|state: &mut DownloadPageState, _| {
                state.plugin_catalog.installing.remove(&plugin.id);
            });
            match result {
                Ok(()) => toast::success(cx, t!("DownloadPage.plugins_install_success")),
                Err(error) => toast::error(cx, error.into()),
            }
        });
    })
    .detach();
}

async fn completed(task: &str) -> Result<String, String> {
    let snapshot = wait_for_task(task).await?;
    let message = snapshot.message.as_deref().unwrap_or_default().to_owned();
    if snapshot.status.as_ref() == "completed" {
        Ok(message)
    } else {
        Err(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::LightColors;

    struct DetailsProbe;

    impl Render for DetailsProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .children(render_details(&LightColors::theme(), window, cx))
        }
    }

    struct CatalogProbe(Entity<BoundedImageCache>);

    impl Render for CatalogProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            render(&LightColors::theme(), cx, &self.0)
        }
    }

    fn sample_plugin(id: &str) -> registry::Plugin {
        registry::Plugin {
            id: id.into(),
            name: "Test Tools".into(),
            description: "Plugin description with enough text to exceed a narrow card. ".repeat(6),
            publisher: "BMCBL".into(),
            status: "official".into(),
            version: "1.0.0".into(),
            github_download_count: Some(42),
            tags: vec!["tools".into(), "networking".into(), "p2p".into()],
            icon: None,
        }
    }

    #[gpui::test]
    fn plugin_catalog_rows_fill_the_list_and_open_details(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(I18n::new());
            cx.set_global(runtime::PluginRegistry::default());
            let mut state = DownloadPageState::default();
            state.tab = DownloadTab::Plugin;
            let mut catalog = registry::Catalog::default();
            catalog.plugins = vec![sample_plugin("first")].into();
            state.plugin_catalog.catalog = Some(catalog);
            cx.set_global(state);
        });
        let (_, visual) = cx.add_window_view(|_, cx| {
            CatalogProbe(BoundedImageCache::new(
                BoundedImageCacheConfig::default(),
                cx,
            ))
        });
        visual.simulate_resize(size(px(972.), px(600.)));
        let wide = visual.debug_bounds("catalog-row-first").unwrap();
        assert!(wide.right() >= px(950.), "{wide:?}");
        assert!(wide.size.height < px(90.), "{wide:?}");

        visual.update(|_, cx| {
            cx.update_global(|state: &mut DownloadPageState, _| {
                state.plugin_catalog.catalog.as_mut().unwrap().plugins =
                    vec![sample_plugin("first"), sample_plugin("second")].into();
            });
        });
        visual.simulate_resize(size(px(640.), px(460.)));
        let first = visual.debug_bounds("catalog-row-first").unwrap();
        let second = visual.debug_bounds("catalog-row-second").unwrap();
        assert!(first.right() >= px(618.), "{first:?}");
        assert_eq!(first.size.height, wide.size.height);
        assert_eq!(first.left(), second.left());
        assert_eq!(first.right(), second.right());
        assert!(first.bottom() < second.top());
        visual.simulate_click(first.center(), Modifiers::default());
        visual.update(|_, cx| {
            let state = cx.global::<DownloadPageState>();
            let details = state.plugin_catalog.details.as_ref().unwrap();
            assert_eq!(details.plugin.id, "first");
            assert_eq!(details.plugin.github_download_count, Some(42));
        });
    }

    #[gpui::test]
    fn plugin_readme_dialog_keeps_the_search_and_list_page(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(I18n::new());
            cx.set_global(runtime::PluginRegistry::default());
            let mut state = DownloadPageState::default();
            state.tab = DownloadTab::Plugin;
            state.search_query = "test-tools".into();
            state.page_index = 2;
            state.plugin_catalog.details = Some(Details {
                plugin: sample_plugin("test-tools"),
                readme: Readme::Ready(Some(parse_document(&"README paragraph.\n\n".repeat(80)))),
                load: None,
            });
            cx.set_global(state);
        });
        let (_, visual) = cx.add_window_view(|_, _| DetailsProbe);
        visual.simulate_resize(size(px(760.), px(460.)));
        let bounds = visual.debug_bounds("plugin-readme-dialog").unwrap();
        assert!(bounds.size.width <= px(728.) && bounds.size.height <= px(396.));
        let close = visual.debug_bounds("plugin-readme-close").unwrap();
        assert!(close.origin.y + close.size.height <= bounds.origin.y + bounds.size.height);
        visual.simulate_click(close.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert!(render_details(&LightColors::theme(), window, cx).is_none());
            let state = cx.global::<DownloadPageState>();
            assert_eq!(state.tab, DownloadTab::Plugin);
            assert_eq!(state.search_query.as_ref(), "test-tools");
            assert_eq!(state.page_index, 2);
        });
    }
}
