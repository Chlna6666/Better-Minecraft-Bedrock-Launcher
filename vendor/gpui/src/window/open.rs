use super::state::ElementVisualTransform;
use super::*;

pub(crate) const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(864.));

fn default_bounds(display_id: Option<DisplayId>, cx: &mut App) -> Bounds<Pixels> {
    const DEFAULT_WINDOW_OFFSET: Point<Pixels> = point(px(0.), px(35.));

    // TODO, BUG: if you open a window with the currently active window
    // on the stack, this will erroneously select the 'unwrap_or_else'
    // code path
    cx.active_window()
        .and_then(|w| w.update(cx, |_, window, _| window.bounds()).ok())
        .map(|mut bounds| {
            bounds.origin += DEFAULT_WINDOW_OFFSET;
            bounds
        })
        .unwrap_or_else(|| {
            let display = display_id
                .map(|id| cx.find_display(id))
                .unwrap_or_else(|| cx.primary_display());

            display
                .map(|display| display.default_bounds())
                .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE))
        })
}

impl Window {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        options: WindowOptions,
        cx: &mut App,
    ) -> Result<Self> {
        let WindowOptions {
            window_bounds,
            titlebar,
            window_icon,
            focus,
            show,
            kind,
            is_movable,
            is_resizable,
            is_minimizable,
            display_id,
            window_background,
            window_corner_preference,
            app_id,
            window_min_size,
            window_decorations,
            #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
            tabbing_identifier,
        } = options;
        let transparent_caption_height = titlebar.as_ref().and_then(|titlebar| {
            titlebar
                .appears_transparent
                .then_some(titlebar.transparent_caption_height.unwrap_or(px(32.0)))
        });
        #[cfg(target_os = "linux")]
        let server_titlebar_fallback = titlebar
            .as_ref()
            .filter(|_| {
                window_decorations.unwrap_or(WindowDecorations::Server) == WindowDecorations::Server
            })
            .map(|titlebar| super::state::ServerTitlebarFallback {
                title: titlebar.title.clone().unwrap_or_default(),
                is_minimizable,
                is_maximizable: is_resizable,
            });

        let bounds = window_bounds
            .map(|bounds| bounds.bounds())
            .unwrap_or_else(|| default_bounds(display_id, cx));
        let mut platform_window = cx.platform.open_window(
            handle,
            WindowParams {
                bounds,
                titlebar,
                window_icon: window_icon.or_else(|| cx.default_window_icon.clone()),
                kind,
                is_movable,
                is_resizable,
                is_minimizable,
                focus,
                show,
                display_id,
                window_background,
                window_min_size,
                window_corner_preference,
                #[cfg(target_os = "macos")]
                tabbing_identifier,
            },
        )?;

        let tab_bar_visible = platform_window.tab_bar_visible();
        SystemWindowTabController::init_visible(cx, tab_bar_visible);
        if let Some(tabs) = platform_window.tabbed_windows() {
            SystemWindowTabController::add_tab(cx, handle.window_id(), tabs);
        }

        let display_id = platform_window.display().map(|display| display.id());
        let sprite_atlas = platform_window.sprite_atlas();
        let mouse_position = platform_window.mouse_position();
        let modifiers = platform_window.modifiers();
        let capslock = platform_window.capslock();
        let content_size = platform_window.content_size();
        let scale_factor = platform_window.scale_factor();
        let appearance = platform_window.appearance();
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "window internals share text shaping state through Arc on the foreground thread"
        )]
        let text_system = Arc::new(WindowTextSystem::new(cx.text_system().clone()));
        let invalidator = WindowInvalidator::new();
        let dirty_frame_diagnostics = Rc::new(RefCell::new(DirtyFrameDiagnostics::default()));
        invalidator.set_dirty_frame_diagnostics(dirty_frame_diagnostics.clone());
        let active = Rc::new(Cell::new(platform_window.is_active()));
        let hovered = Rc::new(Cell::new(platform_window.is_hovered()));
        let needs_present = Rc::new(Cell::new(false));
        let next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>> = Default::default();
        let now = Instant::now();
        let last_input_timestamp = Rc::new(Cell::new(now - Duration::from_secs(2)));
        let async_app = cx.to_async();
        let frame_watchdog = Rc::new(Cell::new(FrameWatchdog::default()));
        let inactive_animation_frame_pending = Rc::new(Cell::new(false));
        let animation_frame_pending_entities = Rc::new(RefCell::new(FxHashSet::default()));
        let animation_engine = Rc::new(RefCell::new(AnimationEngine::new()));
        let deadline_invalidation_pending = Rc::new(RefCell::new(FxHashMap::default()));
        let deadline_invalidation_generation = Rc::new(Cell::new(0));

        platform_window
            .request_decorations(window_decorations.unwrap_or(WindowDecorations::Server));
        platform_window.set_background_appearance(window_background);

        if let Some(ref window_open_state) = window_bounds {
            match window_open_state {
                WindowBounds::Fullscreen(_) => platform_window.toggle_fullscreen(),
                WindowBounds::Maximized(_) => platform_window.zoom(),
                WindowBounds::Windowed(_) => {}
            }
        }

        platform_window.on_close(Box::new({
            let window_id = handle.window_id();
            let mut cx = cx.to_async();
            move || {
                let _ = handle.update(&mut cx, |_, window, _| window.remove_window());
                let _ = cx.update(|cx| {
                    SystemWindowTabController::remove_tab(cx, window_id);
                });
            }
        }));
        platform_window.on_request_frame(Box::new({
            let mut cx = cx.to_async();
            move |frame_options| {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    window.run_platform_frame(frame_options, cx);
                }));
            }
        }));
        platform_window.on_resize(Box::new({
            let mut cx = cx.to_async();
            move |_, _| {
                let _ = ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.content_bounds_changed(cx)),
                );
            }
        }));
        platform_window.on_moved(Box::new({
            let mut cx = cx.to_async();
            move || {
                let _ = ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.window_origin_changed(cx)),
                );
            }
        }));
        platform_window.on_appearance_changed(Box::new({
            let mut cx = cx.to_async();
            move || {
                let _ = ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.appearance_changed(cx)),
                );
            }
        }));
        platform_window.on_active_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    window.active.set(active);
                    if active {
                        window.last_inactive_animation_frame.set(None);
                        window.inactive_animation_frame_pending.set(false);
                        window.frame_throttle.clear_delay();
                    }
                    window.modifiers = window.platform_window.modifiers();
                    window.capslock = window.platform_window.capslock();
                    window
                        .activation_observers
                        .clone()
                        .retain(&(), |callback| callback(window, cx));

                    window.content_bounds_changed(cx);
                    window.refresh();
                    if active {
                        window.rearm_platform_frame_watchdog_on_activation();
                    }

                    SystemWindowTabController::update_last_active(cx, window.handle.id);
                }));
            }
        }));
        platform_window.on_hover_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, _| {
                    window.hovered.set(active);
                    window.refresh();
                }));
            }
        }));
        platform_window.on_input({
            let mut cx = cx.to_async();
            Box::new(move |event| {
                ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.dispatch_event(event, cx)),
                )
                .unwrap_or(DispatchEventResult::default())
            })
        });
        platform_window.on_hit_test_window_control({
            let mut cx = cx.to_async();
            Box::new(move || {
                ignore_window_not_found(handle.update(&mut cx, |_, window, _cx| {
                    for (area, hitbox) in &window.rendered_frame.window_control_hitboxes {
                        if window.mouse_hit_test.ids.contains(&hitbox.id) {
                            return Some(*area);
                        }
                    }
                    None
                }))
                .unwrap_or(None)
            })
        });
        platform_window.on_move_tab_to_new_window({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::move_tab_to_new_window(cx, handle.window_id());
                }));
            })
        });
        platform_window.on_merge_all_windows({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::merge_all_windows(cx, handle.window_id());
                }));
            })
        });
        platform_window.on_select_next_tab({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::select_next_tab(cx, handle.window_id());
                }));
            })
        });
        platform_window.on_select_previous_tab({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::select_previous_tab(cx, handle.window_id())
                }));
            })
        });
        platform_window.on_toggle_tab_bar({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    let tab_bar_visible = window.platform_window.tab_bar_visible();
                    SystemWindowTabController::set_visible(cx, tab_bar_visible);
                }));
            })
        });

        if let Some(app_id) = app_id {
            platform_window.set_app_id(&app_id);
        }

        platform_window.map_window().unwrap();

        let client_inset = if matches!(
            platform_window.window_decorations(),
            Decorations::Client { .. }
        ) && is_resizable
        {
            let inset = platform_window.default_client_inset().unwrap_or(px(8.0));
            platform_window.set_client_inset(inset);
            Some(inset)
        } else {
            None
        };

        Ok(Window {
            handle,
            invalidator,
            removed: false,
            platform_window,
            #[cfg(target_os = "linux")]
            server_titlebar_fallback,
            display_id,
            sprite_atlas,
            text_system,
            default_text_style: cx.default_text_style.clone(),
            image_pipeline_config: cx.image_pipeline_config(),
            trim_memory_on_hidden: false,
            rem_size: px(16.),
            rem_size_override_stack: SmallVec::new(),
            viewport_size: content_size,
            layout_engine: Some(TaffyLayoutEngine::new()),
            root: None,
            element_id_stack: SmallVec::default(),
            text_style_stack: Vec::new(),
            rendered_entity_stack: Vec::new(),
            element_offset_stack: Vec::new(),
            content_mask_stack: Vec::new(),
            visual_content_mask_stack: Vec::new(),
            element_opacity: 1.0,
            element_visual_transform: ElementVisualTransform::identity(),
            requested_autoscroll: None,
            rendered_frame: Frame::new(DispatchTree::new(cx.keymap.clone(), cx.actions.clone())),
            next_frame: Frame::new(DispatchTree::new(cx.keymap.clone(), cx.actions.clone())),
            render_dirty_region: DirtyRegion::empty(),
            animation_dirty_region: DirtyRegion::empty(),
            render_present_mode: PartialPresentMode::FullRedraw,
            render_trim_policy: RetainedResourceTrimPolicy::None,
            force_full_redraw: Cell::new(true),
            force_view_cache_refresh: true,
            idle_render_frames: 0,
            next_frame_callbacks,
            next_hitbox_id: HitboxId(0),
            next_tooltip_id: TooltipId::default(),
            tooltip_bounds: None,
            dirty_views: FxHashSet::default(),
            focus_listeners: SubscriberSet::new(),
            focus_lost_listeners: SubscriberSet::new(),
            default_prevented: true,
            mouse_position,
            mouse_hit_test: HitTest::default(),
            modifiers,
            capslock,
            scale_factor,
            bounds_observers: SubscriberSet::new(),
            appearance,
            appearance_observers: SubscriberSet::new(),
            active,
            hovered,
            needs_present,
            last_input_timestamp,
            animation_time: Cell::new(now),
            refreshing: false,
            dirty_frame_scheduled: false,
            dirty_frame_throttle_pending: false,
            dirty_frame_deferred_pending: false,
            async_app,
            frame_watchdog,
            platform_frame_watchdog_task: None,
            frame_throttle: WindowFrameThrottle::default(),
            draw_deadline: None,
            draw_was_degraded: false,
            recovering_degraded_draw: false,
            last_generation_stats: FrameGenerationStats::default(),
            dirty_frame_diagnostics,
            pending_list_measured_items: 0,
            has_completed_rendered_frame: false,
            critical_draw_depth: 0,
            inactive_animation_frame_pending,
            last_inactive_animation_frame: Rc::new(Cell::new(None)),
            animation_frame_pending_entities,
            animation_engine,
            animation_engine_frame_driver: Cell::new(None),
            image_animation_deadline_pending: Rc::new(RefCell::new(FxHashMap::default())),
            deadline_invalidation_pending,
            deadline_invalidation_generation,
            image_animation_deadline_generation: Rc::new(Cell::new(0)),
            activation_observers: SubscriberSet::new(),
            focus: None,
            focus_enabled: true,
            pending_input: None,
            pending_modifier: ModifierState::default(),
            pending_input_observers: SubscriberSet::new(),
            prompt: None,
            client_inset,
            window_control_drag_gesture: TitlebarGestureState::default(),
            transparent_caption_enabled: transparent_caption_height.is_some(),
            transparent_caption_height,
            observed_caption_height: None,
            image_cache_stack: Vec::new(),
            animated_image_slots: FxHashMap::default(),
            image_paint_tile_cache: FxHashMap::default(),
            #[cfg(any(feature = "inspector", debug_assertions))]
            inspector: None,
        })
    }
}
