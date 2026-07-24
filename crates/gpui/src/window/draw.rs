use super::*;

impl Window {
    /// Produces a new frame and assigns it to `rendered_frame`. To actually show
    /// the contents of the new [`Scene`], use [`Self::present`].
    #[profiling::function]
    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {
        let previous_scene_was_empty = self.rendered_frame.scene.len() == 0;
        let force_full_redraw = self.force_full_redraw.get();
        let restored_input_handler_index = self.begin_draw_cycle(cx);
        self.draw_roots(cx);
        self.next_frame.window_active = self.active.get();

        if self.draw_was_degraded && self.has_completed_rendered_frame {
            return self.finish_degraded_draw(restored_input_handler_index);
        }

        // Register requested input handler with the platform window.
        if let Some(input_handler) = self
            .next_frame
            .input_handlers
            .iter_mut()
            .rev()
            .find_map(|handler| handler.take())
        {
            self.platform_window.set_input_handler(input_handler);
        }

        self.finish_completed_draw(previous_scene_was_empty, force_full_redraw, cx)
    }

    fn begin_draw_cycle(&mut self, cx: &mut App) -> Option<usize> {
        let frame_budget = DIRTY_FRAME_BACKPRESSURE_BUDGET;
        self.dirty_frame_scheduled = false;
        self.draw_deadline = Some(Instant::now() + frame_budget);
        self.draw_was_degraded = false;
        record_window_layout_recompute(self.handle.window_id().as_u64());
        self.invalidate_entities();
        self.pending_list_measured_items = 0;
        cx.entities.clear_accessed();
        debug_assert!(self.rendered_entity_stack.is_empty());
        self.invalidator.set_dirty(false);
        self.requested_autoscroll = None;
        self.restore_previous_input_handler()
    }

    fn restore_previous_input_handler(&mut self) -> Option<usize> {
        let Some(input_handler) = self.platform_window.take_input_handler() else {
            return None;
        };

        if let Some((index, slot)) = self
            .rendered_frame
            .input_handlers
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, handler)| handler.is_none())
        {
            *slot = Some(input_handler);
            Some(index)
        } else {
            let index = self.rendered_frame.input_handlers.len();
            self.rendered_frame.input_handlers.push(Some(input_handler));
            Some(index)
        }
    }

    fn finish_degraded_draw(
        &mut self,
        restored_input_handler_index: Option<usize>,
    ) -> ArenaClearNeeded {
        self.restore_input_handler_after_degraded_draw(restored_input_handler_index);
        self.finish_layout_and_text_frame();
        let frame_retained_capacity = self.next_frame.retained_capacity();
        let scene_metrics = self.next_frame.scene.frame_metrics();
        self.last_generation_stats.scene = scene_metrics;
        self.last_generation_stats.frame_retained_capacity = frame_retained_capacity;
        self.last_generation_stats.list_measured_items = self.pending_list_measured_items;
        record_frame_retained_capacity(frame_retained_capacity);
        record_scene_frame_metrics(scene_metrics);
        self.rendered_frame
            .element_states
            .extend(self.next_frame.element_states.drain());
        self.next_frame.clear();
        self.invalidator.set_dirty(true);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        self.force_full_redraw.set(true);
        self.force_view_cache_refresh = true;
        self.recovering_degraded_draw = true;
        self.draw_deadline = None;
        ArenaClearNeeded
    }

    fn finish_completed_draw(
        &mut self,
        previous_scene_was_empty: bool,
        force_full_redraw: bool,
        cx: &mut App,
    ) -> ArenaClearNeeded {
        self.finish_layout_and_text_frame();
        self.next_frame.finish(&mut self.rendered_frame);
        self.prepare_render_plan_for_next_frame(
            previous_scene_was_empty || force_full_redraw || self.draw_was_degraded,
        );
        let frame_retained_capacity = self.next_frame.retained_capacity();
        let scene_metrics = self.next_frame.scene.frame_metrics();
        self.last_generation_stats.scene = scene_metrics;
        self.last_generation_stats.frame_retained_capacity = frame_retained_capacity;
        self.last_generation_stats.list_measured_items = self.pending_list_measured_items;
        record_frame_retained_capacity(frame_retained_capacity);
        record_scene_frame_metrics(scene_metrics);

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        self.next_frame.clear();
        let current_focus_path = self.rendered_frame.focus_path();
        let current_window_active = self.rendered_frame.window_active;

        self.emit_focus_change_events(
            previous_focus_path,
            previous_window_active,
            current_focus_path,
            current_window_active,
            cx,
        );

        debug_assert!(self.rendered_entity_stack.is_empty());
        self.record_entities_accessed(cx);
        self.reset_cursor_style(cx);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        self.force_full_redraw.set(false);
        self.force_view_cache_refresh = false;
        self.recovering_degraded_draw = false;
        self.has_completed_rendered_frame = true;
        self.needs_present.set(true);
        if self.draw_was_degraded {
            self.invalidator.set_dirty(true);
        } else {
            self.dirty_views.clear();
            self.animation_dirty_region = DirtyRegion::empty();
        }
        self.draw_deadline = None;

        ArenaClearNeeded
    }

    fn finish_layout_and_text_frame(&mut self) {
        let mut layout_metrics = LayoutFrameMetrics::default();
        let mut layout_cache_metrics = LayoutCacheFrameMetrics::default();
        if let Some(layout_engine) = self.layout_engine.as_mut() {
            let (layout_cache_hits, layout_cache_misses) = layout_engine.layout_cache_metrics();
            layout_metrics = layout_engine.frame_metrics();
            layout_cache_metrics = LayoutCacheFrameMetrics {
                hits: layout_cache_hits,
                misses: layout_cache_misses,
            };
            record_layout_frame_metrics(layout_metrics);
            record_layout_cache_metrics(layout_cache_hits, layout_cache_misses);
            layout_engine.clear();
        }
        let text_layout_metrics = self.text_system().finish_frame();
        self.last_generation_stats.layout = layout_metrics;
        self.last_generation_stats.layout_cache = layout_cache_metrics;
        self.last_generation_stats.text_layout = text_layout_metrics;
    }

    fn emit_focus_change_events(
        &mut self,
        previous_focus_path: SmallVec<[FocusId; 8]>,
        previous_window_active: bool,
        current_focus_path: SmallVec<[FocusId; 8]>,
        current_window_active: bool,
        cx: &mut App,
    ) {
        if previous_focus_path != current_focus_path
            || previous_window_active != current_window_active
        {
            if !previous_focus_path.is_empty() && current_focus_path.is_empty() {
                self.focus_lost_listeners
                    .clone()
                    .retain(&(), |listener| listener(self, cx));
            }

            let event = WindowFocusEvent {
                previous_focus_path: if previous_window_active {
                    previous_focus_path
                } else {
                    Default::default()
                },
                current_focus_path: if current_window_active {
                    current_focus_path
                } else {
                    Default::default()
                },
            };
            self.focus_listeners
                .clone()
                .retain(&(), |listener| listener(&event, self, cx));
        }
    }

    fn restore_input_handler_after_degraded_draw(&mut self, restored_index: Option<usize>) {
        let restored_input_handler = restored_index
            .and_then(|index| self.rendered_frame.input_handlers.get_mut(index))
            .and_then(Option::take)
            .or_else(|| {
                self.next_frame
                    .input_handlers
                    .iter_mut()
                    .rev()
                    .find_map(|handler| handler.take())
            });

        if let Some(input_handler) = restored_input_handler {
            self.platform_window.set_input_handler(input_handler);
        }
    }

    fn prepare_render_plan_for_next_frame(&mut self, force_full_redraw: bool) {
        let viewport = Bounds::new(Point::default(), self.viewport_size).scale(self.scale_factor);
        let mut dirty_region = DirtyRegion::empty();

        let scene_requires_full_redraw = self.next_frame.scene.requires_full_redraw_fallback();
        let requires_full_redraw = force_full_redraw || scene_requires_full_redraw;

        if requires_full_redraw {
            if scene_requires_full_redraw {
                crate::diagnostics::performance_metrics::record_full_redraw_fallback();
            }
            dirty_region.mark_full(viewport);
        } else {
            for segment in &self.next_frame.retained_scene_segments {
                if segment.dirty {
                    dirty_region.push(segment.bounds);
                }
            }
            for rect in self.animation_dirty_region.rects() {
                dirty_region.push(rect.bounds);
            }

            if !dirty_region.is_empty() && self.next_frame.scene.has_backdrop_blurs() {
                for bounds in self.next_frame.scene.backdrop_blur_bounds() {
                    dirty_region.push(bounds);
                }
            }

            if dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty() {
                dirty_region.mark_full(viewport);
            } else if !dirty_region.is_empty() {
                dirty_region.coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);
            }
        }

        self.render_present_mode = if dirty_region.is_full()
            || (dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty())
        {
            PartialPresentMode::FullRedraw
        } else {
            PartialPresentMode::Partial
        };
        record_dirty_region_metrics(dirty_region.rect_count(), dirty_region.area() as usize);
        self.render_dirty_region = dirty_region;
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
    }

    fn render_plan(&self) -> FrameRenderPlan<'_> {
        FrameRenderPlan {
            scene: &self.rendered_frame.scene,
            dirty_region: &self.render_dirty_region,
            partial_present_mode: self.render_present_mode,
            trim_policy: self.render_trim_policy,
            visual_effect_quality: FrameVisualEffectQuality::Full,
        }
    }

    fn record_entities_accessed(&mut self, cx: &mut App) {
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        let mut entities = mem::take(entities_ref.deref_mut());
        drop(entities_ref);
        let handle = self.handle;
        cx.record_entities_accessed(
            handle,
            // Try moving window invalidator into the Window
            self.invalidator.clone(),
            &entities,
        );
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        mem::swap(&mut entities, entities_ref.deref_mut());
    }

    fn invalidate_entities(&mut self) {
        let mut views = self.invalidator.take_views();
        for entity in views.drain() {
            self.mark_view_dirty(entity);
        }
        self.invalidator.replace_views(views);
    }

    #[profiling::function]
    pub(super) fn present(&self) {
        self.platform_window.draw(self.render_plan());
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    pub(super) fn present_framebuffer_only(&self) {
        self.platform_window
            .present_framebuffer_only(self.render_plan());
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    fn draw_roots(&mut self, cx: &mut App) {
        self.invalidator.set_phase(DrawPhase::Prepaint);
        self.tooltip_bounds.take();

        let _inspector_width: Pixels = rems(30.0).to_pixels(self.rem_size());
        let root_size = {
            #[cfg(any(feature = "inspector", debug_assertions))]
            {
                self.viewport_size
            }
            #[cfg(not(any(feature = "inspector", debug_assertions)))]
            {
                self.viewport_size
            }
        };

        #[cfg(target_os = "linux")]
        let (mut root_element, wrapper_view_id) = self.linux_root_element();
        #[cfg(not(target_os = "linux"))]
        let (mut root_element, wrapper_view_id) =
            (self.root.as_ref().unwrap().clone().into_any(), None);
        self.with_critical_draw(|window| match wrapper_view_id {
            Some(view_id) => window.with_rendered_view(view_id, |window| {
                root_element.prepaint_as_root(Point::default(), root_size.into(), window, cx);
            }),
            None => {
                root_element.prepaint_as_root(Point::default(), root_size.into(), window, cx);
            }
        });

        #[cfg(any(feature = "inspector", debug_assertions))]
        let inspector_element = if self.inspector.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            None
        } else {
            self.prepaint_inspector(_inspector_width, cx)
        };

        let mut sorted_deferred_draws =
            (0..self.next_frame.deferred_draws.len()).collect::<SmallVec<[_; 8]>>();
        sorted_deferred_draws.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        if !sorted_deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            sorted_deferred_draws.clear();
        } else {
            self.prepaint_deferred_draws(&sorted_deferred_draws, cx);
        }

        let mut prompt_element = None;
        let mut active_drag_element = None;
        let mut tooltip_element = None;
        let has_overlay_work = self.prompt.is_some()
            || cx.active_drag.is_some()
            || !self.next_frame.tooltip_requests.is_empty();
        if has_overlay_work && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(prompt) = self.prompt.take() {
            let mut element = prompt.view.any_view().into_any();
            element.prepaint_as_root(Point::default(), root_size.into(), self, cx);
            prompt_element = Some(element);
            self.prompt = Some(prompt);
        } else if let Some(active_drag) = cx.active_drag.take() {
            let mut element = active_drag.view.clone().into_any();
            let offset = self.mouse_position() - active_drag.cursor_offset;
            element.prepaint_as_root(offset, AvailableSpace::min_size(), self, cx);
            active_drag_element = Some(element);
            cx.active_drag = Some(active_drag);
        } else {
            tooltip_element = self.prepaint_tooltip(cx);
        }

        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);

        self.invalidator.set_phase(DrawPhase::Paint);
        self.with_critical_draw(|window| match wrapper_view_id {
            Some(view_id) => {
                window.with_rendered_view(view_id, |window| root_element.paint(window, cx))
            }
            None => root_element.paint(window, cx),
        });

        #[cfg(any(feature = "inspector", debug_assertions))]
        if inspector_element.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.paint_inspector(inspector_element, cx);
        }

        if !sorted_deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.paint_deferred_draws(&sorted_deferred_draws, cx);
        }

        let has_overlay_element =
            prompt_element.is_some() || active_drag_element.is_some() || tooltip_element.is_some();
        if has_overlay_element && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(mut prompt_element) = prompt_element {
            prompt_element.paint(self, cx);
        } else if let Some(mut drag_element) = active_drag_element {
            drag_element.paint(self, cx);
        } else if let Some(mut tooltip_element) = tooltip_element {
            tooltip_element.paint(self, cx);
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        self.paint_inspector_hitbox(cx);
    }

    #[cfg(target_os = "linux")]
    fn linux_root_element(&self) -> (AnyElement, Option<EntityId>) {
        let decorations = self.platform_window.window_decorations();
        let fullscreen = self.platform_window.is_fullscreen();
        let Some(titlebar) = self
            .server_titlebar_fallback
            .as_ref()
            .filter(|_| should_draw_linux_server_titlebar_fallback(true, decorations, fullscreen))
        else {
            return (self.root.as_ref().unwrap().clone().into_any(), None);
        };

        let wrapper_view_id = self.root.as_ref().unwrap().entity_id();

        let dark = matches!(
            self.appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let active = self.active.get();
        let background = match (dark, active) {
            (true, true) => crate::rgb(0x303030),
            (true, false) => crate::rgb(0x383838),
            (false, true) => crate::rgb(0xf2f2f2),
            (false, false) => crate::rgb(0xf8f8f8),
        };
        let foreground = if dark {
            crate::rgb(0xf2f2f2)
        } else {
            crate::rgb(0x202020)
        };
        let border = if dark {
            crate::rgb(0x1f1f1f)
        } else {
            crate::rgb(0xd8d8d8)
        };
        let button_hover = if dark {
            crate::rgb(0x484848)
        } else {
            crate::rgb(0xe4e4e4)
        };
        let controls = self.platform_window.window_controls();
        let is_maximized = self.platform_window.is_maximized();

        let title = crate::div()
            .id("gpui-linux-server-titlebar-drag")
            .flex()
            .flex_1()
            .h_full()
            .items_center()
            .px_3()
            .overflow_hidden()
            .whitespace_nowrap()
            .window_control_area(WindowControlArea::Drag)
            .on_click(|event, window, _cx| {
                if event.is_right_click() {
                    window.show_window_menu(event.position());
                }
            })
            .child(titlebar.title.clone());

        let minimize = (titlebar.is_minimizable && controls.minimize).then(|| {
            crate::div()
                .id("gpui-linux-server-titlebar-minimize")
                .flex()
                .w(px(46.0))
                .h_full()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |style| style.bg(button_hover))
                .on_click(|_, window, _| window.minimize_window())
                .child("\u{2212}")
        });
        let maximize = (titlebar.is_maximizable && controls.maximize).then(|| {
            crate::div()
                .id("gpui-linux-server-titlebar-maximize")
                .flex()
                .w(px(46.0))
                .h_full()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |style| style.bg(button_hover))
                .on_click(|_, window, _| {
                    if window.is_maximized() {
                        window.restore_window();
                    } else {
                        window.maximize_window();
                    }
                })
                .child(if is_maximized { "\u{2750}" } else { "\u{25a1}" })
        });
        let close = crate::div()
            .id("gpui-linux-server-titlebar-close")
            .flex()
            .w(px(46.0))
            .h_full()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(crate::rgb(0xe81123)).text_color(crate::white()))
            .on_click(|_, window, _| window.remove_window())
            .child("\u{00d7}");

        let titlebar_element = crate::div()
            .flex()
            .flex_none()
            .w_full()
            .h(px(38.0))
            .items_center()
            .bg(background)
            .border_b_1()
            .border_color(border)
            .text_color(foreground)
            .text_size(px(13.0))
            .child(title)
            .children(minimize)
            .children(maximize)
            .child(close);

        let element = crate::div()
            .flex()
            .flex_col()
            .size_full()
            .child(titlebar_element)
            .child(
                crate::div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.root.as_ref().unwrap().clone()),
            )
            .into_any_element();

        (element, Some(wrapper_view_id))
    }

    fn prepaint_tooltip(&mut self, cx: &mut App) -> Option<AnyElement> {
        for tooltip_request_index in (0..self.next_frame.tooltip_requests.len()).rev() {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let Some(Some(tooltip_request)) = self
                .next_frame
                .tooltip_requests
                .get(tooltip_request_index)
                .cloned()
            else {
                log::error!("Unexpectedly absent TooltipRequest");
                continue;
            };
            let mut element = tooltip_request.tooltip.view.clone().into_any();
            let mouse_position = tooltip_request.tooltip.mouse_position;
            let tooltip_size = element.layout_as_root(AvailableSpace::min_size(), self, cx);

            let mut tooltip_bounds =
                Bounds::new(mouse_position + point(px(1.), px(1.)), tooltip_size);
            let window_bounds = Bounds {
                origin: Point::default(),
                size: self.viewport_size(),
            };

            if tooltip_bounds.right() > window_bounds.right() {
                let new_x = mouse_position.x - tooltip_bounds.size.width - px(1.);
                if new_x >= Pixels::ZERO {
                    tooltip_bounds.origin.x = new_x;
                } else {
                    tooltip_bounds.origin.x = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.x - tooltip_bounds.right() - window_bounds.right(),
                    );
                }
            }

            if tooltip_bounds.bottom() > window_bounds.bottom() {
                let new_y = mouse_position.y - tooltip_bounds.size.height - px(1.);
                if new_y >= Pixels::ZERO {
                    tooltip_bounds.origin.y = new_y;
                } else {
                    tooltip_bounds.origin.y = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.y - tooltip_bounds.bottom() - window_bounds.bottom(),
                    );
                }
            }

            let is_visible =
                (tooltip_request.tooltip.check_visible_and_update)(tooltip_bounds, self, cx);
            if !is_visible {
                continue;
            }

            self.with_absolute_element_offset(tooltip_bounds.origin, |window| {
                element.prepaint(window, cx)
            });

            self.tooltip_bounds = Some(TooltipBounds {
                id: tooltip_request.id,
                bounds: tooltip_bounds,
            });
            return Some(element);
        }
        None
    }
}

#[cfg(target_os = "linux")]
fn should_draw_linux_server_titlebar_fallback(
    has_fallback: bool,
    decorations: Decorations,
    fullscreen: bool,
) -> bool {
    has_fallback && matches!(decorations, Decorations::Client { .. }) && !fullscreen
}

#[cfg(all(test, target_os = "linux"))]
mod linux_server_titlebar_fallback_tests {
    use super::*;

    #[test]
    fn draws_when_server_decorations_fall_back_to_client_side() {
        assert!(should_draw_linux_server_titlebar_fallback(
            true,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            false,
        ));
    }

    #[test]
    fn stays_hidden_for_server_decorations_fullscreen_or_no_fallback() {
        assert!(!should_draw_linux_server_titlebar_fallback(
            true,
            Decorations::Server,
            false,
        ));
        assert!(!should_draw_linux_server_titlebar_fallback(
            true,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            true,
        ));
        assert!(!should_draw_linux_server_titlebar_fallback(
            false,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            false,
        ));
    }
}
