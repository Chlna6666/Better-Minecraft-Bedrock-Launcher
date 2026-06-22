use super::*;
use gpui::{BoundedImageCache, BoundedImageCacheConfig, Task};
use std::collections::HashSet;

pub(super) const RESULT_LOGO_BYTES_PER_ITEM: usize = 384 * 1024;

pub(crate) struct CurseForgeResultsListView {
    pub(crate) _subscriptions: Vec<Subscription>,
    pub(crate) cached_page_card_props: Vec<CurseForgeResultCardProps>,
    pub(crate) result_logo_cache: Entity<BoundedImageCache>,
    pub(crate) result_image_prefetch_task: Option<Task<anyhow::Result<()>>>,
    pub(crate) result_image_notify_task: Option<Task<anyhow::Result<()>>>,
    pub(crate) result_logo_reveal_task: Option<Task<anyhow::Result<()>>>,
    pub(crate) visible_revealed_logo_urls: HashSet<SharedString>,
    pub(crate) last_observed_tab: DownloadTab,
    pub(crate) last_observed_view_epoch: u64,
    pub(crate) last_observed_page_index: usize,
    pub(crate) last_observed_mod_count: usize,
    pub(crate) last_observed_results_loading: bool,
    pub(crate) last_observed_visible_slice_start: usize,
    pub(crate) last_observed_visible_slice_len: usize,
    pub(crate) last_observed_result_image_change_seq: u64,
    pub(crate) last_prepared_results_signature: (u64, usize, usize, usize, usize),
    pub(crate) last_image_work_signature: (u64, usize, usize, usize, bool, bool, bool, usize),
    pub(crate) last_image_prefetch_signature: (u64, usize, usize, usize, bool, bool, bool, usize),
}

impl CurseForgeResultsListView {
    fn release_cached_result_cards(&mut self) {
        self.cached_page_card_props.clear();
        self.result_image_prefetch_task.take();
        self.result_image_notify_task.take();
        self.result_logo_reveal_task.take();
        self.visible_revealed_logo_urls.clear();
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

    pub(crate) fn sync_visible_result_logo_reveal(&mut self, cx: &mut Context<Self>) {
        let had_visible_reveals = !self.visible_revealed_logo_urls.is_empty();
        self.result_logo_reveal_task.take();
        self.visible_revealed_logo_urls.clear();
        if had_visible_reveals {
            cx.notify();
        }
    }

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
        self.result_image_prefetch_task.take();
        self.last_image_prefetch_signature = image_work_signature;
    }

    fn schedule_image_refresh_notify(&mut self, cx: &mut Context<Self>) {
        self.result_image_notify_task.take();
        cx.notify();
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let (
            last_observed_tab,
            last_observed_view_epoch,
            last_observed_page_index,
            last_observed_mod_count,
            last_observed_results_loading,
            last_observed_visible_slice_start,
            last_observed_visible_slice_len,
        ) = cx.read_global(|state: &DownloadPageState, _cx| {
            (
                state.tab,
                state.curseforge_view_epoch,
                state.curseforge_page_index,
                state.curseforge_mods.len(),
                state.curseforge_results_loading,
                0,
                state.curseforge_mods.len(),
            )
        });
        let last_observed_result_image_change_seq = 0;

        let subscriptions = vec![
            cx.observe_global::<DownloadPageState>(|this, cx| {
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
                    if current_tab == DownloadTab::ResourcePack {
                        this.sync_result_images(cx);
                        cx.notify();
                    }
                }

                let (page_index, mod_count, results_loading, visible_slice_start, visible_slice_len) = cx
                    .read_global(|state: &DownloadPageState, _cx| {
                        (
                            state.curseforge_page_index,
                            state.curseforge_mods.len(),
                            state.curseforge_results_loading,
                            0,
                            state.curseforge_mods.len(),
                        )
                    });

                if current_tab == DownloadTab::ResourcePack
                    && (page_index != this.last_observed_page_index
                        || mod_count != this.last_observed_mod_count
                        || results_loading != this.last_observed_results_loading
                        || visible_slice_start != this.last_observed_visible_slice_start
                        || visible_slice_len != this.last_observed_visible_slice_len)
                {
                    let previous_page_index = this.last_observed_page_index;
                    let previous_mod_count = this.last_observed_mod_count;
                    let previous_results_loading = this.last_observed_results_loading;
                    let previous_visible_slice_start = this.last_observed_visible_slice_start;
                    let previous_visible_slice_len = this.last_observed_visible_slice_len;

                    this.last_observed_page_index = page_index;
                    this.last_observed_mod_count = mod_count;
                    this.last_observed_results_loading = results_loading;
                    this.last_observed_visible_slice_start = visible_slice_start;
                    this.last_observed_visible_slice_len = visible_slice_len;

                    tracing::debug!(
                        "curseforge results window update: page_index={} mod_count={} results_loading={} visible_slice_start={} visible_slice_len={}",
                        page_index,
                        mod_count,
                        results_loading,
                        visible_slice_start,
                        visible_slice_len
                    );

                    let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
                        clamp_curseforge_results_scroll_in_state(state)
                    });

                    let scroll_driven_only = current_tab == DownloadTab::ResourcePack
                        && page_index == previous_page_index
                        && mod_count == previous_mod_count
                        && results_loading == previous_results_loading
                        && (visible_slice_start != previous_visible_slice_start
                            || visible_slice_len != previous_visible_slice_len);

                    if scroll_driven_only {
                        this.sync_result_images(cx);
                        this.sync_visible_result_logo_reveal(cx);
                    } else {
                        this.sync_result_images(cx);
                        this.sync_visible_result_logo_reveal(cx);
                    }
                    cx.notify();
                } else if current_tab != DownloadTab::ResourcePack {
                    this.sync_result_images(cx);
                    this.sync_visible_result_logo_reveal(cx);
                }
            }),
        ];

        let mut this = Self {
            _subscriptions: subscriptions,
            cached_page_card_props: Vec::new(),
            result_logo_cache: BoundedImageCache::new(BoundedImageCacheConfig::default(), cx),
            result_image_prefetch_task: None,
            result_image_notify_task: None,
            result_logo_reveal_task: None,
            visible_revealed_logo_urls: HashSet::new(),
            last_observed_tab,
            last_observed_view_epoch,
            last_observed_page_index,
            last_observed_mod_count,
            last_observed_results_loading,
            last_observed_visible_slice_start,
            last_observed_visible_slice_len,
            last_observed_result_image_change_seq,
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
        this.sync_visible_result_logo_reveal(cx);
        this
    }
}

impl Render for CurseForgeResultsListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = std::time::Instant::now();
        let theme = cx.global::<crate::ui::state::theme::ThemeState>();
        let colors = crate::ui::theme::colors::lerp_theme_colors(
            &crate::ui::theme::colors::LightColors::colors(),
            &crate::ui::theme::colors::DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        render_curseforge_results_list(self, &colors, window, cx)
    }
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
        .rounded(px(9.))
        .bg(Hsla {
            a: 0.10,
            ..colors.surface
        })
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(
            lucide_icons::icon_image(),
            16.0,
            colors.text_muted,
        ))
        .into_any_element()
}

fn curseforge_results_skeleton_bar(colors: &ThemeColors, width: Pixels, height: Pixels) -> Div {
    div().w(width).h(height).rounded(px(999.)).bg(Hsla {
        a: 0.10,
        ..colors.text_secondary
    })
}

fn curseforge_results_skeleton_card(colors: &ThemeColors) -> Div {
    div()
        .w_full()
        .h(px(78.))
        .min_h(px(78.))
        .rounded(px(14.))
        .bg(Hsla {
            a: 0.98,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .px(px(12.))
        .py(px(9.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().w(px(42.)).h(px(42.)).rounded(px(9.)).bg(Hsla {
            a: 0.08,
            ..colors.text_secondary
        }))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(curseforge_results_skeleton_bar(colors, px(220.), px(13.)))
                .child(curseforge_results_skeleton_bar(colors, px(360.), px(11.)))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .min_w(px(0.))
                        .overflow_hidden()
                        .child(curseforge_results_skeleton_bar(colors, px(120.), px(10.)))
                        .child(curseforge_results_skeleton_bar(colors, px(80.), px(10.)))
                        .child(curseforge_results_skeleton_bar(colors, px(92.), px(10.))),
                ),
        )
        .child(div().w(px(92.)).h(px(32.)).rounded(px(10.)).bg(Hsla {
            a: 0.10,
            ..colors.accent
        }))
}

fn curseforge_results_skeleton_row(colors: &ThemeColors) -> Div {
    div()
        .w_full()
        .h(px(super::CURSEFORGE_RESULT_CARD_PITCH_PX))
        .min_h(px(super::CURSEFORGE_RESULT_CARD_PITCH_PX))
        .flex()
        .items_start()
        .child(curseforge_results_skeleton_card(colors))
}

pub(crate) fn render_curseforge_loading_placeholder(colors: &ThemeColors) -> Div {
    let skeleton_bar = |width: Pixels, height: Pixels| {
        div().w(width).h(height).rounded(px(999.)).bg(Hsla {
            a: 0.08,
            ..colors.text_secondary
        })
    };

    let skeleton_shimmer = || {
        div()
            .absolute()
            .top(px(0.))
            .bottom(px(0.))
            .w(px(140.))
            .bg(Hsla {
                a: 0.24,
                ..colors.surface
            })
            .with_animation(
                "curseforge-skeleton-shimmer",
                Animation::new(Duration::from_millis(1400)).repeat(),
                |this, t| this.left(px(-180.0 + t * 440.0)),
            )
            .into_any_element()
    };

    let skeleton_card = || {
        div()
            .w_full()
            .rounded(px(14.))
            .bg(Hsla {
                a: 0.90,
                ..colors.surface
            })
            .border_1()
            .border_color(Hsla {
                a: 0.10,
                ..colors.border
            })
            .px(px(12.))
            .py(px(10.))
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().w(px(42.)).h(px(42.)).rounded(px(9.)).bg(Hsla {
                a: 0.10,
                ..colors.text_secondary
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(skeleton_bar(px(250.), px(14.)))
                    .child(skeleton_bar(px(420.), px(10.)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .min_w(px(0.))
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(skeleton_bar(px(112.), px(10.)))
                                    .child(skeleton_bar(px(84.), px(18.)))
                                    .child(skeleton_bar(px(76.), px(10.))),
                            )
                            .child(skeleton_bar(px(90.), px(10.))),
                    ),
            )
            .child(div().w(px(92.)).h(px(32.)).rounded(px(10.)).bg(Hsla {
                a: 0.10,
                ..colors.accent
            }))
            .child(skeleton_shimmer())
    };

    div()
        .size_full()
        .rounded(px(12.))
        .border_1()
        .border_color(Hsla {
            a: 0.06,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.85,
            ..colors.surface
        })
        .overflow_hidden()
        .min_w(px(0.))
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(16.))
                .py(px(14.))
                .border_1()
                .border_color(Hsla {
                    a: 0.06,
                    ..colors.border
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(skeleton_bar(px(84.), px(20.)))
                                .child(skeleton_bar(px(72.), px(20.)))
                                .child(skeleton_bar(px(52.), px(20.))),
                        )
                        .child(div().w(px(88.)).h(px(28.)).rounded(px(999.)).bg(Hsla {
                            a: 0.06,
                            ..colors.text_secondary
                        })),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(skeleton_card())
                .child(skeleton_card())
                .child(skeleton_card())
                .child(skeleton_card()),
        )
        .child(
            div()
                .px(px(16.))
                .py(px(12.))
                .border_1()
                .border_color(Hsla {
                    a: 0.06,
                    ..colors.border
                })
                .child(div().w_full().h(px(32.)).rounded(px(10.)).bg(Hsla {
                    a: 0.05,
                    ..colors.text_secondary
                })),
        )
}

pub(crate) fn render_curseforge_results_list_placeholder(colors: &ThemeColors) -> Div {
    let skeleton_bar = |width: Pixels, height: Pixels| {
        div().w(width).h(height).rounded(px(999.)).bg(Hsla {
            a: 0.08,
            ..colors.text_secondary
        })
    };

    let skeleton_card = || {
        div()
            .w_full()
            .rounded(px(14.))
            .bg(Hsla {
                a: 0.90,
                ..colors.surface
            })
            .border_1()
            .border_color(Hsla {
                a: 0.10,
                ..colors.border
            })
            .px(px(12.))
            .py(px(10.))
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().w(px(42.)).h(px(42.)).rounded(px(9.)).bg(Hsla {
                a: 0.10,
                ..colors.text_secondary
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(skeleton_bar(px(250.), px(14.)))
                    .child(skeleton_bar(px(420.), px(10.)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .min_w(px(0.))
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(skeleton_bar(px(112.), px(10.)))
                                    .child(skeleton_bar(px(84.), px(18.)))
                                    .child(skeleton_bar(px(76.), px(10.))),
                            )
                            .child(skeleton_bar(px(90.), px(10.))),
                    ),
            )
            .child(div().w(px(92.)).h(px(32.)).rounded(px(10.)).bg(Hsla {
                a: 0.10,
                ..colors.accent
            }))
    };

    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap(px(12.))
        .px(px(16.))
        .py(px(16.))
        .child(skeleton_card())
        .child(skeleton_card())
        .child(skeleton_card())
        .child(skeleton_card())
        .child(skeleton_card())
}

pub(crate) fn render_curseforge_results_list_placeholder_aligned(
    colors: &ThemeColors,
    state: &DownloadPageState,
) -> Div {
    let viewport_height_px = state.curseforge_results_scroll.bounds().size.height / px(1.0);
    let pitch_px = super::CURSEFORGE_RESULT_CARD_PITCH_PX.max(1.0);
    let skeleton_count = ((viewport_height_px / pitch_px).ceil() as usize)
        .saturating_add(1)
        .clamp(3, 8);

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
            (0..skeleton_count).map(|_| curseforge_results_skeleton_row(colors).into_any_element()),
        )
}
