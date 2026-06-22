use crate::ui::views::download::state::DownloadPageState;
use gpui::*;
use std::time::Duration;

pub(crate) fn invalidate_results_in_state(state: &mut DownloadPageState, cx: &mut App) {
    state.curseforge_page_commit_task.take();
    state.curseforge_pending_page_index = None;
    if let Some(handle) = state.curseforge_results_abort_handle.take() {
        handle.abort();
    }
    state.curseforge_results_epoch = state.curseforge_results_epoch.wrapping_add(1);
    state.curseforge_results_loading = true;
    state.curseforge_results_error = None;
    state.curseforge_last_query_key = SharedString::from("");
    state.curseforge_results_transition_at = None;
    state.curseforge_mods.clear();
    state.curseforge_mods.shrink_to_fit();
    state.curseforge_mod_page_open = false;
    state.curseforge_mod_page_loading = false;
    state.curseforge_mod_page_error = None;
    state.curseforge_mod_page_mod_id = None;
    state.curseforge_mod_page_mod = None;
    state.set_curseforge_mod_page_description(SharedString::from(""));
    state.curseforge_disable_result_logos = true;
    // 关键：分类切换时不立即重置滚动条，等待新数据加载完成后再重置
    // state.curseforge_results_scroll.set_offset(point(px(0.), px(0.)));
    state.curseforge_pending_scroll_reset_to_top = true;
    // 不要在旧列表还可见时先跳顶；等新页真正提交时再一起置顶
    state.curseforge_pending_scroll_reset_to_top = true;
}

pub(crate) fn invalidate_results_now_in_state(state: &mut DownloadPageState, cx: &mut App) {
    state.curseforge_invalidate_task.take();
    state.curseforge_invalidate_seq = state.curseforge_invalidate_seq.wrapping_add(1);
    invalidate_results_in_state(state, cx);
}

pub(crate) fn begin_page_results_transition_in_state(state: &mut DownloadPageState, cx: &mut App) {
    if let Some(handle) = state.curseforge_results_abort_handle.take() {
        handle.abort();
    }
    state.curseforge_results_epoch = state.curseforge_results_epoch.wrapping_add(1);
    state.curseforge_results_loading = true;
    state.curseforge_mods.clear();
    state.curseforge_mods.shrink_to_fit();
    state.curseforge_results_error = None;
    state.curseforge_disable_result_logos = true;
    // 关键：翻页时不立即重置滚动条，等待新数据加载完成后再重置
    // state.curseforge_results_scroll.set_offset(point(px(0.), px(0.)));
    state.curseforge_pending_scroll_reset_to_top = true;
    // 翻页开始时保留旧页位置；等新页提交时再一起置顶
    state.curseforge_pending_scroll_reset_to_top = true;
    state.curseforge_results_transition_at = None;
}

pub(crate) fn schedule_invalidate_results_in_state(state: &mut DownloadPageState, cx: &mut App) {
    state.curseforge_invalidate_task.take();
    state.curseforge_invalidate_seq = state.curseforge_invalidate_seq.wrapping_add(1);
    let seq = state.curseforge_invalidate_seq;
    let task = cx.spawn(async move |cx| {
        Timer::after(Duration::from_millis(120)).await;
        let should_load = match cx.update_global(|state: &mut DownloadPageState, cx| {
            if state.curseforge_invalidate_seq != seq {
                return false;
            }
            state.curseforge_invalidate_task = None;
            invalidate_results_in_state(state, cx);
            true
        }) {
            Ok(should_load) => should_load,
            Err(error) => {
                tracing::trace!("curseforge invalidate task update skipped: {error}");
                false
            }
        };
        let _ = should_load;
    });

    state.curseforge_invalidate_task = Some(task);
}

pub(crate) fn apply_results_query_change_in_state(
    state: &mut DownloadPageState,
    cx: &mut App,
    update_state: impl FnOnce(&mut DownloadPageState) -> bool,
) {
    if !update_state(state) {
        return;
    }

    // 关键：root/sub/search/sort 变化和翻页统一走同一个静止窗口
    state.curseforge_disable_result_logos = true;

    invalidate_results_now_in_state(state, cx);
}

pub(crate) fn ensure_results_loaded(force_refresh: bool, cx: &mut App) {
    ensure_results_loaded_impl(force_refresh, false, None, cx);
}

pub(crate) fn ensure_results_loaded_after_page_transition(
    force_refresh: bool,
    target_page_index: usize,
    cx: &mut App,
) {
    ensure_results_loaded_impl(force_refresh, true, Some(target_page_index), cx);
}

fn ensure_results_loaded_impl(
    force_refresh: bool,
    state_already_invalidated: bool,
    page_index_override: Option<usize>,
    cx: &mut App,
) {
    let (
        invalidate_pending,
        is_loading,
        last_key,
        key,
        current_mod_count,
        page_index,
        class_id,
        category_id,
        game_version,
        search_filter,
        sort_field,
        sort_order,
        page_size,
        curseforge_view_epoch,
        curseforge_results_epoch,
    ) = cx.read_global(|state: &DownloadPageState, _cx| {
        let trimmed_search_query = state.search_query.trim();
        let trimmed_game_version = state.curseforge_selected_game_version.trim();
        let page_index = page_index_override.unwrap_or(state.curseforge_page_index);
        let key = format!(
            "root={:?};sub={:?};ver={};sort={};order={};q={};p={};ps={}",
            state.curseforge_selected_root_id,
            state.curseforge_selected_sub_id,
            trimmed_game_version,
            state.curseforge_sort_field,
            state.curseforge_sort_order,
            trimmed_search_query,
            state.curseforge_page_index,
            state.curseforge_page_size
        );
        (
            state.curseforge_invalidate_task.is_some(),
            state.curseforge_results_loading,
            state.curseforge_last_query_key.to_string(),
            key,
            state.curseforge_mods.len(),
            page_index,
            state.curseforge_selected_root_id,
            state.curseforge_selected_sub_id,
            trimmed_game_version.to_string(),
            trimmed_search_query.to_string(),
            state.curseforge_sort_field,
            state.curseforge_sort_order.to_string(),
            state.curseforge_page_size,
            state.curseforge_view_epoch,
            state.curseforge_results_epoch,
        )
    });

    if invalidate_pending && !force_refresh {
        return;
    }
    if is_loading && !force_refresh && last_key == key {
        return;
    }
    if !force_refresh && last_key == key {
        return;
    }

    let state_already_invalidated = state_already_invalidated || last_key.is_empty();

    if last_key != key && !state_already_invalidated {
        let _ = cx.update_global(|state: &mut DownloadPageState, cx| {
            invalidate_results_in_state(state, cx);
        });
    }

    let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.curseforge_results_loading = true;
        state.curseforge_results_error = None;
        state.curseforge_last_query_key = SharedString::from(key.clone());
    });

    let (tx, rx) = tokio::sync::oneshot::channel();
    let target_page_index = page_index;
    let task = tokio::spawn(async move {
        let index = (target_page_index as u32).saturating_mul(page_size);
        let result: Result<
            (
                Vec<crate::ui::views::download::state::CurseForgeModEntry>,
                Option<u32>,
                bool,
            ),
            String,
        > = async {
            let client = crate::core::curseforge::CurseForgeClient::new()?;
            let mut query = crate::core::curseforge::SearchModsQuery::default();
            query.class_id = class_id;
            query.category_id = category_id;
            query.game_version = if game_version.trim().is_empty() {
                None
            } else {
                Some(game_version)
            };
            query.search_filter = if search_filter.trim().is_empty() {
                None
            } else {
                Some(search_filter)
            };
            query.sort_field = Some(sort_field);
            query.sort_order = Some(sort_order);
            query.page_size = Some(page_size);
            query.index = Some(index);

            let response = client.search_mods(query).await?;
            let mods = response
                .data
                .into_iter()
                .map(
                    |mod_entry| crate::ui::views::download::state::CurseForgeModEntry {
                        id: mod_entry.id,
                        name: SharedString::from(mod_entry.name),
                        summary: mod_entry.summary.map(SharedString::from),
                        author_names: mod_entry
                            .authors
                            .into_iter()
                            .map(|author| SharedString::from(author.name))
                            .collect(),
                        logo_url: mod_entry
                            .logo
                            .map(|logo| SharedString::from(logo.thumbnail_url.unwrap_or(logo.url))),
                        download_count: mod_entry.download_count,
                        date_modified: SharedString::from(mod_entry.date_modified),
                        class_id: mod_entry.class_id,
                        category_ids: mod_entry
                            .categories
                            .into_iter()
                            .map(|category| category.id)
                            .collect(),
                    },
                )
                .collect::<Vec<_>>();

            let total_count = response
                .pagination
                .and_then(|pagination| pagination.total_count);
            let has_more = if let Some(total_count) = total_count {
                index.saturating_add(mods.len() as u32) < total_count
            } else {
                mods.len() as u32 >= page_size
            };

            Ok((mods, total_count, has_more))
        }
        .await;
        let _ = tx.send(result);
    });

    let abort_handle = task.abort_handle();
    let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
        if let Some(handle) = state.curseforge_results_abort_handle.replace(abort_handle) {
            handle.abort();
        }
    });

    cx.spawn(async move |cx| {
        let result = rx
            .await
            .map_err(|_| "curseforge search task dropped".to_string());

        match result {
            Ok(Ok((mods, total_count, has_more))) => {
                match cx.update_global(|state: &mut DownloadPageState, cx| {
                    state.curseforge_results_abort_handle = None;
                    if state.curseforge_view_epoch != curseforge_view_epoch {
                        return false;
                    }
                    if state.curseforge_results_epoch != curseforge_results_epoch {
                        return false;
                    }

                    state.curseforge_page_index = target_page_index;
                    state.curseforge_mods = mods;
                    state.curseforge_total_count = total_count;
                    state.curseforge_has_more = has_more;
                    state.curseforge_results_loading = false;
                    state.curseforge_results_error = None;
                    state.curseforge_disable_result_logos = false;
                    state.curseforge_results_transition_at = Some(std::time::Instant::now());
                    state.curseforge_pending_page_index = None;

                    if state.curseforge_pending_scroll_reset_to_top {
                        state.curseforge_pending_scroll_reset_to_top = false;
                        state
                            .curseforge_results_scroll
                            .set_offset(point(px(0.), px(0.)));
                    }

                    // 关键：不再在这里 prune 图片缓存，让列表视图自己根据可视区管理
                    true
                }) {
                    Ok(true) => {
                        if let Err(error) = cx.refresh() {
                            tracing::trace!("curseforge results success refresh skipped: {error}");
                        }
                    }
                    Ok(false) => {}
                    Err(error) => tracing::warn!("curseforge results update failed: {error:?}"),
                }
            }
            Ok(Err(error_message)) | Err(error_message) => {
                match cx.update_global(|state: &mut DownloadPageState, _cx| {
                    state.curseforge_results_abort_handle = None;
                    if state.curseforge_view_epoch != curseforge_view_epoch {
                        return false;
                    }
                    if state.curseforge_results_epoch != curseforge_results_epoch {
                        return false;
                    }
                    state.curseforge_results_loading = false;
                    state.curseforge_results_error =
                        Some(SharedString::from(error_message.clone()));
                    state.curseforge_disable_result_logos = false;
                    state.curseforge_pending_page_index = None;
                    true
                }) {
                    Ok(true) => {
                        if let Err(error) = cx.refresh() {
                            tracing::trace!("curseforge results error refresh skipped: {error}");
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!("curseforge results error update failed: {error:?}")
                    }
                }
            }
        }

        Ok::<(), ()>(())
    })
    .detach();
}
