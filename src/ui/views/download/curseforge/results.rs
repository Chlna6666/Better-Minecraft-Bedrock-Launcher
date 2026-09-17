use super::*;
use crate::ui::components::virtual_list::{VirtualListConfig, VirtualListSlice};
use gpui::{AssetLocation, BoundedImageCache, BoundedImageCacheConfig};

pub(super) const RESULT_LOGO_BYTES_PER_ITEM: usize = 384 * 1024;
const RESULT_LOGO_PREFETCH_LOOKAHEAD: usize = 2;
const RESULT_PROJECTION_RETAIN_ROWS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResultProjectionGeneration {
    results_epoch: u64,
    view_epoch: u64,
    page_index: usize,
    mod_count: usize,
    i18n_revision: u64,
}

pub(crate) struct CurseForgeResultsListView {
    pub(crate) _subscriptions: Vec<Subscription>,
    // Compatibility storage for the previous page-wide renderer. The active renderer below keeps
    // this empty and stores only a small retained projection window instead.
    pub(crate) cached_page_card_props: Vec<CurseForgeResultCardProps>,
    cached_window_card_props: Vec<CurseForgeResultCardProps>,
    cached_window_slice: VirtualListSlice,
    cached_window_generation: Option<ResultProjectionGeneration>,
    pub(crate) result_logo_cache: Entity<BoundedImageCache>,
    pub(crate) last_observed_tab: DownloadTab,
    pub(crate) last_observed_view_epoch: u64,
    pub(crate) last_observed_page_index: usize,
    pub(crate) last_observed_mod_count: usize,
    pub(crate) last_observed_results_loading: bool,
    pub(crate) last_observed_visible_slice_start: usize,
    pub(crate) last_observed_visible_slice_len: usize,
    pub(crate) last_prepared_results_signature: (u64, usize, usize, usize, usize),
    pub(crate) last_image_work_signature: (u64, usize, usize, usize, bool, bool, bool, usize),
    pub(crate) last_image_prefetch_signature: (u64, usize, usize, usize, bool, bool, bool, usize),
}

fn result_logo_cache_config(max_items: usize) -> BoundedImageCacheConfig {
    BoundedImageCacheConfig {
        max_items,
        max_bytes: max_items.saturating_mul(RESULT_LOGO_BYTES_PER_ITEM),
    }
}

impl CurseForgeResultsListView {
    fn release_cached_result_cards(&mut self) {
        // `clear` deliberately keeps capacity. This path is a lifecycle release point, so replacing
        // the vectors is preferable: memory from a previously large result page can return to the
        // allocator while the list is inactive.
        self.cached_page_card_props = Vec::new();
        self.cached_window_card_props = Vec::new();
        self.cached_window_slice = VirtualListSlice::default();
        self.cached_window_generation = None;
        self.last_observed_page_index = usize::MAX;
        self.last_observed_mod_count = 0;
        self.last_observed_results_loading = false;
        self.last_observed_visible_slice_start = usize::MAX;
        self.last_observed_visible_slice_len = 0;
        self.last_prepared_results_signature = (u64::MAX, usize::MAX, 0, usize::MAX, 0);
        self.last_image_work_signature = (u64::MAX, usize::MAX, usize::MAX, 0, true, true, true, 0);
        self.last_image_prefetch_signature =
            (u64::MAX, usize::MAX, usize::MAX, 0, true, true, true, 0);
    }

    pub(crate) fn sync_visible_result_logo_reveal(&mut self, _cx: &mut Context<Self>) {}

    pub(crate) fn sync_result_images(&mut self, cx: &mut Context<Self>) {
        let image_work_signature = cx.read_global(|state: &DownloadPageState, _cx| {
            (
                state.curseforge_results_epoch,
                state.curseforge_page_index,
                0,
                state.curseforge_mods.len(),
                state.curseforge_results_loading,
                state.curseforge_pending_page_index.is_some(),
                state.curseforge_disable_result_logos,
                state.curseforge_mods.len(),
            )
        });

        if self.last_image_work_signature == image_work_signature {
            return;
        }

        self.last_image_work_signature = image_work_signature;
        self.last_image_prefetch_signature =
            (u64::MAX, usize::MAX, usize::MAX, 0, true, true, true, 0);
    }

    fn prefetch_visible_result_logos(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_signature = self.last_image_prefetch_signature;
        let (signature, urls) = cx.read_global(|state: &DownloadPageState, _cx| {
            let mod_count = state.curseforge_mods.len();
            let enabled = state.tab == DownloadTab::ResourcePack
                && !state.curseforge_results_loading
                && state.curseforge_pending_page_index.is_none()
                && !state.curseforge_disable_result_logos
                && should_render_curseforge_result_images()
                && should_mount_curseforge_result_images();
            let plan = VirtualListConfig::new(CURSEFORGE_RESULT_CARD_PITCH_PX)
                .with_heavy_budget(if enabled {
                    CURSEFORGE_RESULT_LOGO_RENDER_BUDGET
                } else {
                    0
                })
                .plan_for_scroll_handle(mod_count, &state.curseforge_results_scroll);
            let start = plan.heavy_slice.start_index.min(mod_count);
            let len = if enabled {
                plan.heavy_slice
                    .len()
                    .saturating_add(RESULT_LOGO_PREFETCH_LOOKAHEAD)
                    .min(mod_count.saturating_sub(start))
            } else {
                0
            };
            let signature = (
                state.curseforge_results_epoch,
                state.curseforge_page_index,
                start,
                len,
                state.curseforge_results_loading,
                state.curseforge_pending_page_index.is_some(),
                state.curseforge_disable_result_logos,
                mod_count,
            );
            let urls = if enabled && signature != previous_signature {
                state
                    .curseforge_mods
                    .iter()
                    .skip(start)
                    .take(len)
                    .filter_map(|entry| entry.logo_url.clone())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            (signature, urls)
        });

        if previous_signature == signature {
            return;
        }
        self.last_image_prefetch_signature = signature;
        if urls.is_empty() {
            return;
        }

        self.result_logo_cache.update(cx, |cache, cx| {
            for url in urls {
                let source = AssetLocation::Uri(url.into());
                let _ = cache.load(&source, window, cx);
            }
        });
    }

    fn ensure_projection_window(
        &mut self,
        generation: ResultProjectionGeneration,
        required_slice: VirtualListSlice,
        i18n: &I18n,
        cx: &mut Context<Self>,
    ) {
        if required_slice.is_empty() {
            self.cached_window_card_props = Vec::new();
            self.cached_window_slice = VirtualListSlice::default();
            self.cached_window_generation = Some(generation);
            return;
        }

        let cache_contains_required = self.cached_window_generation == Some(generation)
            && self.cached_window_slice.start_index <= required_slice.start_index
            && self.cached_window_slice.end_index >= required_slice.end_index
            && self.cached_window_card_props.len() == self.cached_window_slice.len();
        if cache_contains_required {
            return;
        }

        // Keep lightweight row projections around the actual render window, but never GPUI row
        // elements. This hysteresis prevents string/category projection work on every one-row scroll
        // boundary while keeping memory O(viewport), independent of the result page size.
        let cache_start = required_slice
            .start_index
            .saturating_sub(RESULT_PROJECTION_RETAIN_ROWS);
        let cache_end = required_slice
            .end_index
            .saturating_add(RESULT_PROJECTION_RETAIN_ROWS)
            .min(generation.mod_count);
        let cache_slice = VirtualListSlice {
            start_index: cache_start,
            end_index: cache_end,
        };
        let next = cx.read_global(|state: &DownloadPageState, _cx| {
            build_curseforge_result_card_props(state, i18n, cache_slice.start_index, cache_slice.len())
        });

        self.cached_window_card_props = next;
        self.cached_window_slice = cache_slice;
        self.cached_window_generation = Some(generation);
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let (
            last_observed_tab,
            last_observed_view_epoch,
            last_observed_page_index,
            last_observed_mod_count,
            last_observed_results_loading,
        ) = cx.read_global(|state: &DownloadPageState, _cx| {
            (
                state.tab,
                state.curseforge_view_epoch,
                state.curseforge_page_index,
                state.curseforge_mods.len(),
                state.curseforge_results_loading,
            )
        });

        let subscriptions = vec![cx.observe_global::<DownloadPageState>(|this, cx| {
            let (current_tab, current_view_epoch) =
                cx.read_global(|state: &DownloadPageState, _cx| {
                    (state.tab, state.curseforge_view_epoch)
                });

            if current_view_epoch != this.last_observed_view_epoch {
                this.release_cached_result_cards();
                this.last_observed_view_epoch = current_view_epoch;
            }

            if current_tab != this.last_observed_tab {
                this.last_observed_tab = current_tab;
                this.release_cached_result_cards();
                // The next visible render grows the cache only to its viewport budget. Replacing
                // the entity here also releases the list's ownership of images from the inactive
                // tab instead of retaining the default 256-item cache indefinitely.
                this.result_logo_cache =
                    BoundedImageCache::new(result_logo_cache_config(0), cx);
                if current_tab == DownloadTab::ResourcePack {
                    this.sync_result_images(cx);
                }
                cx.notify();
            }

            let (page_index, mod_count, results_loading) =
                cx.read_global(|state: &DownloadPageState, _cx| {
                    (
                        state.curseforge_page_index,
                        state.curseforge_mods.len(),
                        state.curseforge_results_loading,
                    )
                });

            if current_tab == DownloadTab::ResourcePack
                && (page_index != this.last_observed_page_index
                    || mod_count != this.last_observed_mod_count
                    || results_loading != this.last_observed_results_loading)
            {
                this.last_observed_page_index = page_index;
                this.last_observed_mod_count = mod_count;
                this.last_observed_results_loading = results_loading;
                this.cached_window_generation = None;

                let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    clamp_curseforge_results_scroll_in_state(state)
                });
                this.sync_result_images(cx);
                this.sync_visible_result_logo_reveal(cx);
                cx.notify();
            }
        })];

        let mut this = Self {
            _subscriptions: subscriptions,
            cached_page_card_props: Vec::new(),
            cached_window_card_props: Vec::new(),
            cached_window_slice: VirtualListSlice::default(),
            cached_window_generation: None,
            result_logo_cache: BoundedImageCache::new(result_logo_cache_config(0), cx),
            last_observed_tab,
            last_observed_view_epoch,
            last_observed_page_index,
            last_observed_mod_count,
            last_observed_results_loading,
            last_observed_visible_slice_start: usize::MAX,
            last_observed_visible_slice_len: 0,
            last_prepared_results_signature: (u64::MAX, usize::MAX, 0, usize::MAX, 0),
            last_image_work_signature: (u64::MAX, usize::MAX, usize::MAX, 0, true, true, true, 0),
            last_image_prefetch_signature: (
                u64::MAX,
                usize::MAX,
                usize::MAX,
                0,
                true,
                true,
                true,
                0,
            ),
        };
        this.sync_result_images(cx);
        this
    }
}

impl Render for CurseForgeResultsListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let theme = cx.global::<crate::ui::state::theme::ThemeState>();
        let colors = crate::ui::theme::colors::lerp_theme_colors(
            &crate::ui::theme::colors::LightColors::colors(),
            &crate::ui::theme::colors::DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let content = render_virtualized_curseforge_results_list(self, &colors, window, cx);
        self.prefetch_visible_result_logos(window, cx);
        content
    }
}

fn render_virtualized_curseforge_results_list(
    this: &mut CurseForgeResultsListView,
    colors: &ThemeColors,
    window: &mut Window,
    cx: &mut Context<CurseForgeResultsListView>,
) -> Div {
    let frame_now = window.animation_time();
    let i18n = cx.global::<I18n>().clone();
    let i18n_revision = i18n.revision();
    let (
        results_loading,
        results_error,
        disable_result_logos,
        results_epoch,
        view_epoch,
        results_transition_at,
        pending_page_index,
        page_index,
        mod_count,
    ) = cx.read_global(|state: &DownloadPageState, _cx| {
        (
            state.curseforge_results_loading,
            state.curseforge_results_error.clone(),
            state.curseforge_disable_result_logos,
            state.curseforge_results_epoch,
            state.curseforge_view_epoch,
            state.curseforge_results_transition_at,
            state.curseforge_pending_page_index,
            state.curseforge_page_index,
            state.curseforge_mods.len(),
        )
    });

    let list = {
        let results_scroll = cx
            .global::<DownloadPageState>()
            .curseforge_results_scroll
            .clone();
        div()
            .size_full()
            .min_h(px(0.))
            .id("curseforge-results-scroll")
            .overflow_y_scroll()
            .track_scroll(&cx.global::<DownloadPageState>().curseforge_results_scroll)
            .scrollbar_width(px(0.))
            .on_scroll_wheel(move |event, window, cx| {
                let offset = results_scroll.offset();
                let max_offset = results_scroll.max_offset();
                let delta_y = scroll_event_delta_y_with_line_height(event, window.line_height());
                let at_bottom = offset.y <= -max_offset.height;
                let at_top = offset.y >= px(0.);

                if (at_bottom && delta_y < Pixels::ZERO) || (at_top && delta_y > Pixels::ZERO) {
                    results_scroll
                        .set_offset(point(offset.x, offset.y.clamp(-max_offset.height, px(0.))));
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .px(px(12.))
            .py(px(12.))
            .flex()
            .flex_col()
    };

    // Loading/paging placeholders replace the list entirely. Do not build row projections, mount
    // images, or allocate hidden card trees whose output would immediately be covered by the overlay.
    if results_loading || pending_page_index.is_some() {
        let state = cx.global::<DownloadPageState>();
        return render_curseforge_results_list_placeholder_aligned(colors, state);
    }

    if let Some(err) = results_error.as_ref() {
        return div().size_full().child(list.child(status_card(
            colors,
            &t!("CurseForge.load_failed", error = err),
            Some(colors.danger),
        )));
    }

    if mod_count == 0 {
        return div().size_full().child(list.child(status_card(
            colors,
            &t!("CurseForge.no_results"),
            None,
        )));
    }

    let animate_cards = should_animate_curseforge_result_cards();
    let reveal_warmup_pending = if !animate_cards {
        false
    } else if let Some(started_at) = results_transition_at {
        frame_now.saturating_duration_since(started_at).as_millis() as u64
            < CURSEFORGE_RESULTS_REVEAL_WARMUP_MS
    } else {
        false
    };
    if reveal_warmup_pending {
        let warmup_deadline = results_transition_at.map(|started_at| {
            started_at + Duration::from_millis(CURSEFORGE_RESULTS_REVEAL_WARMUP_MS)
        });
        crate::ui::animation::request_layout_animation_frame_until_active(window, warmup_deadline);
        let state = cx.global::<DownloadPageState>();
        return render_curseforge_results_list_placeholder_aligned(colors, state);
    }

    // Element overscan is deliberately zero: offscreen rows are not materialized at all. A small
    // projection-only retention window in `ensure_projection_window` provides scroll hysteresis
    // without paying GPUI layout/prepaint/paint or event-handler memory for invisible cards.
    let virtual_list_plan = cx.read_global(|state: &DownloadPageState, _cx| {
        VirtualListConfig::new(CURSEFORGE_RESULT_CARD_PITCH_PX)
            .with_heavy_budget(if disable_result_logos {
                0
            } else {
                CURSEFORGE_RESULT_LOGO_RENDER_BUDGET
            })
            .plan_for_scroll_handle(mod_count, &state.curseforge_results_scroll)
    });

    let generation = ResultProjectionGeneration {
        results_epoch,
        view_epoch,
        page_index,
        mod_count,
        i18n_revision,
    };
    this.ensure_projection_window(
        generation,
        VirtualListSlice {
            start_index: virtual_list_plan.render_slice.start_index,
            end_index: virtual_list_plan.render_slice.end_index,
        },
        &i18n,
        cx,
    );

    let logo_cache_items = if disable_result_logos {
        0
    } else {
        virtual_list_plan
            .heavy_slice
            .len()
            .saturating_add(RESULT_LOGO_PREFETCH_LOOKAHEAD)
            .min(mod_count)
    };
    this.result_logo_cache.update(cx, |cache, cx| {
        cache.update_limits(result_logo_cache_config(logo_cache_items), window, cx);
    });

    let render_started_at = std::time::Instant::now();
    let mut visible_card_items = div().w_full().flex().flex_col().gap(px(6.));
    let default_install_target = default_install_target_for_results(cx);

    for virtual_index in virtual_list_plan.render_range() {
        let Some(cache_index) = virtual_index.checked_sub(this.cached_window_slice.start_index)
        else {
            continue;
        };
        let Some(cached_card_props) = this.cached_window_card_props.get(cache_index) else {
            continue;
        };
        let is_visible = virtual_list_plan.visible_slice.contains(virtual_index);
        let visible_order = virtual_index.saturating_sub(virtual_list_plan.visible_slice.start_index);
        let transition_started_at = if is_visible { results_transition_at } else { None };
        let is_heavy_card = !disable_result_logos
            && virtual_list_plan.heavy_slice.contains(virtual_index);

        visible_card_items = visible_card_items.child(render_curseforge_result_card(
            colors,
            &i18n,
            cached_card_props,
            &this.result_logo_cache,
            default_install_target.clone(),
            is_heavy_card,
            transition_started_at,
            frame_now,
            visible_order,
        ));
    }

    // Only rows that can actually be seen extend the layout-animation lifetime. Offscreen indices
    // must never keep the window in a repaint loop.
    let transition_animating = animate_cards
        && results_transition_at.is_some_and(|started_at| {
            let visible_count = virtual_list_plan.visible_slice.len() as u64;
            if visible_count == 0 {
                return false;
            }
            let total_duration_ms = CURSEFORGE_RESULT_CARD_ANIMATION_MS
                + visible_count.saturating_sub(1) * CURSEFORGE_RESULT_CARD_STAGGER_MS;
            (frame_now
                .saturating_duration_since(started_at)
                .as_millis() as u64)
                < total_duration_ms.max(CURSEFORGE_RESULTS_TRANSITION_MS)
        });
    crate::ui::animation::request_layout_animation_frame_if(window, transition_animating);

    let content = div()
        .size_full()
        .relative()
        .overflow_hidden()
        .child(list.child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(div().h(virtual_list_plan.render_slice.top_spacer))
                .child(visible_card_items)
                .child(div().h(virtual_list_plan.render_slice.bottom_spacer)),
        ));

    let render_elapsed_ms = render_started_at.elapsed().as_secs_f64() * 1000.0;
    if render_elapsed_ms >= 8.0 {
        tracing::debug!(
            "curseforge results render slow: elapsed_ms={render_elapsed_ms:.3} page_index={} total={} render_start={} render_len={} projection_start={} projection_len={} heavy_len={} logo_cache_items={}",
            page_index,
            mod_count,
            virtual_list_plan.render_slice.start_index,
            virtual_list_plan.render_slice.visible_len(),
            this.cached_window_slice.start_index,
            this.cached_window_slice.len(),
            virtual_list_plan.heavy_slice.len(),
            logo_cache_items,
        );
    }

    content
}

#[derive(Clone, PartialEq)]
pub(crate) struct CurseForgeResultCardProps {
    pub(crate) mod_id: i32,
    pub(crate) title: SharedString,
    pub(crate) summary: SharedString,
    pub(crate) authors: SharedString,
    pub(crate) primary_tag_label: Option<SharedString>,
    pub(crate) logo_url: Option<SharedString>,
    pub(crate) download_count_label: SharedString,
    pub(crate) date_modified_label: SharedString,
}

pub(crate) fn render_result_logo_placeholder(colors: ThemeColors) -> AnyElement {
    div()
        .w(px(42.))
        .h(px(42.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.10,
            ..colors.surface
        })
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(
            lucide_gpui::icon!(image),
            16.0,
            colors.text_muted,
        ))
        .into_any_element()
}

fn curseforge_results_skeleton_row(colors: &ThemeColors) -> Div {
    let bar = |width: Pixels, height: Pixels| {
        div()
            .w(width)
            .h(height)
            .rounded(px(crate::ui::theme::tokens::radius::FULL))
            .bg(Hsla {
                a: 0.08,
                ..colors.text_secondary
            })
    };

    div()
        .w_full()
        .h(px(CURSEFORGE_RESULT_CARD_PITCH_PX))
        .min_h(px(CURSEFORGE_RESULT_CARD_PITCH_PX))
        .flex()
        .items_start()
        .child(
            div()
                .w_full()
                .h(px(78.))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .px(px(12.))
                .py(px(9.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(42.))
                        .h(px(42.))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.text_secondary
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(bar(px(220.), px(13.)))
                        .child(bar(px(360.), px(11.)))
                        .child(bar(px(120.), px(10.))),
                )
                .child(bar(px(92.), px(30.))),
        )
}

pub(crate) fn render_curseforge_results_list_placeholder_aligned(
    colors: &ThemeColors,
    state: &DownloadPageState,
) -> Div {
    let viewport_height_px = state.curseforge_results_scroll.bounds().size.height / px(1.0);
    let pitch_px = CURSEFORGE_RESULT_CARD_PITCH_PX.max(1.0);
    let skeleton_count = if viewport_height_px.is_finite() && viewport_height_px > 0.0 {
        ((viewport_height_px / pitch_px).ceil() as usize).clamp(1, 8)
    } else {
        4
    };

    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_hidden()
        .px(px(12.))
        .py(px(12.))
        .flex()
        .flex_col()
        .children(
            (0..skeleton_count)
                .map(|_| curseforge_results_skeleton_row(colors).into_any_element()),
        )
}
