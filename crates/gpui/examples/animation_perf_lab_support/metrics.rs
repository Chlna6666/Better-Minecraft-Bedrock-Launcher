use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{
    WindowAnimationPresentationTiming, WindowAnimationSamples, WindowBackendReadyMetricsSnapshot,
    WindowVSyncMetricsSnapshot, performance_metrics_snapshot, window_animation_samples_since,
    window_metrics_snapshot,
};

use crate::{
    Config, GATE_WINDOW, UI_RENDER_BLOCK, backend_label, duration_ms, duration_us, micros_to_millis,
};

pub(crate) struct GateBaseline {
    pub(crate) window_id: u64,
    pub(crate) sample_rate: f64,
    pub(crate) sample_interval_p50_micros: u64,
    pub(crate) sample_sequence: u64,
    pub(crate) changed_sample_count: u64,
    pub(crate) unchanged_sample_count: u64,
    pub(crate) skipped_frame_count: u64,
    pub(crate) native_vsync: Option<WindowVSyncMetricsSnapshot>,
    pub(crate) backend_ready_wake_count: usize,
    pub(crate) started_at: Instant,
    pub(crate) render_count: usize,
}

pub(crate) enum BlockEvent {
    Started(GateBaseline),
    Finished {
        ended_at: Instant,
        render_count: usize,
        native_vsync: Option<WindowVSyncMetricsSnapshot>,
        backend_ready: Option<WindowBackendReadyMetricsSnapshot>,
    },
}

pub(crate) fn observe(
    config: Config,
    receiver: mpsc::Receiver<BlockEvent>,
    render_count: Arc<AtomicUsize>,
) {
    let baseline = match receiver.recv_timeout(GATE_WINDOW) {
        Ok(BlockEvent::Started(baseline)) => baseline,
        Ok(BlockEvent::Finished { .. }) => {
            eprintln!("FAIL: received the UI Render block end before it started");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("FAIL: the 200 ms UI Render block did not start within {GATE_WINDOW:?}");
            std::process::exit(1);
        }
    };

    let Some(mut baseline_window) = window_sample(baseline.window_id) else {
        eprintln!("FAIL: no metrics were available for the animation window");
        std::process::exit(1);
    };
    baseline_window.animation_sampled_present_count = baseline.sample_sequence;
    baseline_window.animation_sample_changed_present_count = baseline.changed_sample_count;
    baseline_window.animation_sample_unchanged_present_count = baseline.unchanged_sample_count;
    baseline_window.skipped_frame_count = baseline.skipped_frame_count;
    let renders_before = baseline.render_count;
    let mut previous_window = baseline_window;
    let mut middle_window = baseline_window;
    let mut slice_samples = Vec::with_capacity(5);
    let mut slices_continuous = true;
    let mut complete_slices = 0usize;
    let max_sample_gap_micros = max_sample_gap(baseline.sample_interval_p50_micros);

    let block_started_at = baseline.started_at;
    let mut slice_started_at = block_started_at;
    let mut second_half_started = false;
    let mut block_finished = false;
    let mut block_finished_at = None;
    let mut block_render_count = None;
    let mut block_native_vsync_after = None;
    let mut block_backend_ready_after = None;
    while !block_finished {
        let event = receiver.recv_timeout(Duration::from_millis(40));
        if matches!(&event, Ok(BlockEvent::Started(_))) {
            slices_continuous = false;
            eprintln!("FAIL: received a duplicate UI Render block start event");
        }
        let Some(current_window) = window_sample(baseline.window_id) else {
            slices_continuous = false;
            eprintln!("FAIL: animation window metrics disappeared during the UI Render block");
            break;
        };
        let samples = current_window
            .animation_sampled_present_count
            .saturating_sub(previous_window.animation_sampled_present_count);
        let changed = current_window
            .animation_sample_changed_present_count
            .saturating_sub(previous_window.animation_sample_changed_present_count);
        let unchanged = current_window
            .animation_sample_unchanged_present_count
            .saturating_sub(previous_window.animation_sample_unchanged_present_count);
        let slice_elapsed = slice_started_at.elapsed();
        let complete_slice = slice_elapsed >= Duration::from_millis(30);
        slice_samples.push((samples, changed, unchanged, slice_elapsed.as_millis()));
        if complete_slice {
            complete_slices = complete_slices.saturating_add(1);
            let minimum = minimum_samples(baseline.sample_rate, slice_elapsed);
            slices_continuous &= samples >= minimum && changed == samples && unchanged == 0;
        } else {
            // A final event can wake the observer before another full sampling interval elapses.
            // The overall presentation count and maximum inter-sample gap cover this partial tail.
            slices_continuous &= changed == samples && unchanged == 0;
        }
        previous_window = current_window;
        slice_started_at = Instant::now();

        let elapsed = block_started_at.elapsed();
        if !second_half_started && elapsed >= UI_RENDER_BLOCK / 2 {
            middle_window = current_window;
            second_half_started = true;
        }
        if let Ok(BlockEvent::Finished {
            ended_at,
            render_count,
            native_vsync,
            backend_ready,
        }) = &event
        {
            block_finished = true;
            block_finished_at = Some(*ended_at);
            block_render_count = Some(*render_count);
            block_native_vsync_after = native_vsync.clone();
            block_backend_ready_after = backend_ready.clone();
        }
        if matches!(&event, Err(mpsc::RecvTimeoutError::Disconnected)) {
            slices_continuous = false;
            eprintln!("FAIL: UI Render block event channel closed before the block ended");
            break;
        }
    }

    let after = counter_snapshot();
    let renders_after = render_count.load(Ordering::Relaxed);
    let first_half_samples = middle_window
        .animation_sampled_present_count
        .saturating_sub(baseline_window.animation_sampled_present_count);
    let second_half_samples = previous_window
        .animation_sampled_present_count
        .saturating_sub(middle_window.animation_sampled_present_count);
    let skipped_frames = previous_window
        .skipped_frame_count
        .saturating_sub(baseline_window.skipped_frame_count);
    let block_finished_at = block_finished_at.unwrap_or_else(Instant::now);
    let block_duration = block_finished_at.saturating_duration_since(block_started_at);
    let block_samples =
        window_animation_samples_since(baseline.window_id, baseline.sample_sequence);
    let block_continuity =
        block_sample_continuity(block_samples.as_ref(), block_started_at, block_finished_at);
    let expected = expected_samples(baseline.sample_rate, block_duration);
    let minimum = expected.saturating_mul(90).div_ceil(100).max(2);
    let sample_count_passed = block_continuity.sample_count as u64 >= minimum;
    let samples_changed_passed = block_continuity.unchanged_count == 0
        && block_continuity.changed_count == block_continuity.sample_count;
    let sample_gap_passed = block_continuity.max_gap_micros < max_sample_gap_micros;
    let sample_history_passed = !block_continuity.history_overflowed;
    let ui_render_passed = block_render_count == Some(renders_before);
    let gate_passed = slices_continuous
        && complete_slices >= 3
        && sample_count_passed
        && samples_changed_passed
        && first_half_samples > 0
        && second_half_samples > 0
        && sample_gap_passed
        && sample_history_passed
        && ui_render_passed;
    let worst_gap_timing = block_continuity.worst_gap_timing.unwrap_or_default();
    let worst_gap_backend_timing_ms = backend_timing_ms(worst_gap_timing.backend_timings);
    let worst_gap_renderer_timing_ms = renderer_timing_ms(Some(worst_gap_timing));
    let backend_timing_max_ms = block_continuity
        .backend_timing_max_micros
        .map(|micros| micros_to_millis(micros as usize));
    println!(
        "{}: 200 ms UI Render block; window={} elapsed_ms={} sample_target={} active_samples={} changed={} unchanged={} (first/second half {}/{}) 40ms_slices(samples/changed/unchanged@ms)={:?} complete_slices={} block_gap_max={:.3}ms limit={:.3}ms block_boundary_gaps_ms[start/end]={:.3}/{:.3} history_overflowed={} checks[slices/count/changed/gap/history/ui]={}/{}/{}/{}/{}/{} timing_samples={} stage_max_ms[pacing/queue/dispatch/present]={:.3}/{:.3}/{:.3}/{:.3} backend_timing_samples={} backend_stage_max_ms[fence/acquire/submit/wait/present]={:?} worst_gap_after_seq={:?} worst_gap_stage_ms[pacing/queue/dispatch/present]={:.3}/{:.3}/{:.3}/{:.3} worst_gap_renderer_ms[scene/submit/resources/frame/present/post]={:?} worst_gap_backend_ms[fence/acquire/submit/wait/present]={:?} skipped={} UI Render calls {} -> {} rolling_gap_max={:.3}ms",
        if gate_passed { "PASS" } else { "FAIL" },
        baseline.window_id,
        block_duration.as_millis(),
        expected,
        block_continuity.sample_count,
        block_continuity.changed_count,
        block_continuity.unchanged_count,
        first_half_samples,
        second_half_samples,
        slice_samples,
        complete_slices,
        micros_to_millis(block_continuity.max_gap_micros as usize),
        micros_to_millis(max_sample_gap_micros as usize),
        micros_to_millis(block_continuity.start_gap_micros as usize),
        micros_to_millis(block_continuity.end_gap_micros as usize),
        block_continuity.history_overflowed,
        slices_continuous,
        sample_count_passed,
        samples_changed_passed,
        sample_gap_passed,
        sample_history_passed,
        ui_render_passed,
        block_continuity.timing_sample_count,
        micros_to_millis(block_continuity.timing_max_micros[0] as usize),
        micros_to_millis(block_continuity.timing_max_micros[1] as usize),
        micros_to_millis(block_continuity.timing_max_micros[2] as usize),
        micros_to_millis(block_continuity.timing_max_micros[3] as usize),
        block_continuity.backend_timing_sample_count,
        backend_timing_max_ms,
        block_continuity.worst_gap_after_sequence,
        micros_to_millis(duration_micros(worst_gap_timing.frame_pacing_wait) as usize),
        micros_to_millis(duration_micros(worst_gap_timing.vsync_event_queue_delay) as usize),
        micros_to_millis(duration_micros(worst_gap_timing.window_dispatch_delay) as usize),
        micros_to_millis(duration_micros(worst_gap_timing.active_present_duration) as usize),
        worst_gap_renderer_timing_ms,
        worst_gap_backend_timing_ms,
        skipped_frames,
        renders_before,
        block_render_count.unwrap_or(renders_after),
        micros_to_millis(previous_window.animation_sample_interval_max_micros as usize),
    );
    println!(
        "  worst_gap_renderer_substages_ms[draw/buffer/atlas/offscreen]={:?}",
        renderer_substage_timing_ms(worst_gap_timing),
    );
    println!(
        "  worst_gap_backend_command_ms[encoder/record/fence_reset/cleanup]={:?}",
        backend_command_timing_ms(worst_gap_timing.backend_timings),
    );
    print_native_vsync_gate_delta(
        baseline.native_vsync.as_ref(),
        block_native_vsync_after.as_ref(),
    );
    print_backend_ready_gate_delta_and_final_queue_delay(
        baseline.backend_ready_wake_count,
        block_backend_ready_after.as_ref(),
    );
    if !gate_passed {
        eprintln!(
            "The gate checks successful active-animation presentations inside the exact UI Render block, every sample changing, start/end boundary coverage, maximum in-block gap below 1.8x baseline cadence, and zero UI Render calls during the block."
        );
    }

    let mut continuity_passed = gate_passed;
    let visual_start = Instant::now();
    let visual_start_window = previous_window;
    let visual_start_counters = after;
    let visual_start_renders = renders_after;
    thread::sleep(Duration::from_millis(850));
    let visual_end = Instant::now();
    let visual_window = window_sample(baseline.window_id).unwrap_or(previous_window);
    let visual_counters = counter_snapshot();
    let visual_renders = render_count.load(Ordering::Relaxed);
    let visual_duration = visual_end.saturating_duration_since(visual_start);
    continuity_passed &= print_interval_report(
        "visual-only",
        &baseline,
        max_sample_gap_micros,
        visual_start,
        visual_end,
        visual_start_window,
        visual_window,
        visual_start_counters,
        visual_counters,
        visual_start_renders,
        visual_renders,
        false,
    );
    print_snapshot(
        "visual-only snapshot",
        config.copies,
        visual_counters,
        visual_renders,
        baseline.window_id,
    );

    let remaining = config
        .measurement
        .saturating_sub(UI_RENDER_BLOCK + visual_duration);
    let mut interval_start = Instant::now();
    let mut interval_window = visual_window;
    let mut interval_counters = visual_counters;
    let mut interval_renders = visual_renders;
    let mut monitored = Duration::ZERO;
    while monitored < remaining {
        let interval = (remaining - monitored).min(Duration::from_secs(1));
        thread::sleep(interval);
        monitored += interval;
        let interval_end = Instant::now();
        let current_window = window_sample(baseline.window_id).unwrap_or(interval_window);
        let current_counters = counter_snapshot();
        let current_renders = render_count.load(Ordering::Relaxed);
        continuity_passed &= print_interval_report(
            "sustained",
            &baseline,
            max_sample_gap_micros,
            interval_start,
            interval_end,
            interval_window,
            current_window,
            interval_counters,
            current_counters,
            interval_renders,
            current_renders,
            config.visual_only,
        );
        interval_start = Instant::now();
        interval_window = current_window;
        interval_counters = current_counters;
        interval_renders = current_renders;
    }

    let final_window = window_sample(baseline.window_id).unwrap_or(interval_window);
    print_snapshot(
        "final",
        config.copies,
        counter_snapshot(),
        render_count.load(Ordering::Relaxed),
        baseline.window_id,
    );
    println!(
        "measurement complete: backend={} elapsed_ms={} copies_per_track={} frame_continuity={}",
        backend_label(config.backend),
        (UI_RENDER_BLOCK + visual_duration + monitored).as_millis(),
        config.copies,
        if continuity_passed { "PASS" } else { "FAIL" },
    );
    if final_window.skipped_frame_count > baseline_window.skipped_frame_count {
        println!(
            "window frame skip decisions during run: {}",
            final_window
                .skipped_frame_count
                .saturating_sub(baseline_window.skipped_frame_count)
        );
    }
    if !continuity_passed {
        std::process::exit(1);
    }
}

#[derive(Clone, Copy)]
struct WindowSample {
    window_id: u64,
    present_count: u64,
    animation_sampled_present_count: u64,
    animation_sample_changed_present_count: u64,
    animation_sample_unchanged_present_count: u64,
    animation_sample_interval_p50_micros: u64,
    animation_sample_interval_p95_micros: u64,
    animation_sample_interval_p99_micros: u64,
    animation_sample_interval_max_micros: u64,
    animation_sample_interval_sample_count: u64,
    skipped_frame_count: u64,
    frame_duration_p50_micros: u64,
    frame_duration_p95_micros: u64,
    frame_duration_p99_micros: u64,
    present_interval_p50_micros: u64,
    present_interval_p95_micros: u64,
    present_interval_p99_micros: u64,
    frame_duration_sample_count: u64,
    present_interval_sample_count: u64,
    scale_factor_milli: u64,
    active: bool,
    visible: bool,
    minimized: bool,
}

fn window_sample(window_id: u64) -> Option<WindowSample> {
    window_metrics_snapshot()
        .into_iter()
        .find(|window| window.window_id == window_id)
        .map(|window| WindowSample {
            window_id: window.window_id,
            present_count: window.present_count as u64,
            animation_sampled_present_count: window.animation_sampled_present_count as u64,
            animation_sample_changed_present_count: window.animation_sample_changed_present_count
                as u64,
            animation_sample_unchanged_present_count: window
                .animation_sample_unchanged_present_count
                as u64,
            animation_sample_interval_p50_micros: window.animation_sample_interval_p50_micros
                as u64,
            animation_sample_interval_p95_micros: window.animation_sample_interval_p95_micros
                as u64,
            animation_sample_interval_p99_micros: window.animation_sample_interval_p99_micros
                as u64,
            animation_sample_interval_max_micros: window.animation_sample_interval_max_micros
                as u64,
            animation_sample_interval_sample_count: window.animation_sample_interval_sample_count
                as u64,
            skipped_frame_count: window.skipped_frame_count as u64,
            frame_duration_p50_micros: window.frame_duration_p50_micros as u64,
            frame_duration_p95_micros: window.frame_duration_p95_micros as u64,
            frame_duration_p99_micros: window.frame_duration_p99_micros as u64,
            present_interval_p50_micros: window.present_interval_p50_micros as u64,
            present_interval_p95_micros: window.present_interval_p95_micros as u64,
            present_interval_p99_micros: window.present_interval_p99_micros as u64,
            frame_duration_sample_count: window.frame_duration_sample_count as u64,
            present_interval_sample_count: window.present_interval_sample_count as u64,
            scale_factor_milli: window.scale_factor_milli as u64,
            active: window.active,
            visible: window.visible,
            minimized: window.minimized,
        })
}

fn expected_samples(sample_rate: f64, duration: Duration) -> u64 {
    (sample_rate * duration.as_secs_f64())
        .ceil()
        .clamp(0.0, u64::MAX as f64) as u64
}

#[derive(Default)]
struct BlockSampleContinuity {
    sample_count: usize,
    changed_count: usize,
    unchanged_count: usize,
    max_gap_micros: u64,
    start_gap_micros: u64,
    end_gap_micros: u64,
    worst_gap_after_sequence: Option<u64>,
    worst_gap_timing: Option<gpui::WindowAnimationPresentationTiming>,
    history_overflowed: bool,
    timing_sample_count: usize,
    timing_max_micros: [u64; 4],
    backend_timing_sample_count: usize,
    backend_timing_max_micros: [u64; 5],
}

fn block_sample_continuity(
    history: Option<&WindowAnimationSamples>,
    started_at: Instant,
    ended_at: Instant,
) -> BlockSampleContinuity {
    let Some(history) = history else {
        return BlockSampleContinuity {
            max_gap_micros: duration_micros(ended_at.saturating_duration_since(started_at)),
            start_gap_micros: duration_micros(ended_at.saturating_duration_since(started_at)),
            end_gap_micros: duration_micros(ended_at.saturating_duration_since(started_at)),
            history_overflowed: true,
            ..BlockSampleContinuity::default()
        };
    };

    let samples = history
        .samples
        .iter()
        .filter(|sample| sample.presented_at >= started_at && sample.presented_at <= ended_at)
        .collect::<Vec<_>>();
    let Some(first) = samples.first() else {
        let elapsed_micros = duration_micros(ended_at.saturating_duration_since(started_at));
        return BlockSampleContinuity {
            max_gap_micros: elapsed_micros,
            start_gap_micros: elapsed_micros,
            end_gap_micros: elapsed_micros,
            history_overflowed: history.history_overflowed,
            ..BlockSampleContinuity::default()
        };
    };
    let last = samples.last().unwrap_or(first);
    let start_gap_micros =
        duration_micros(first.presented_at.saturating_duration_since(started_at));
    let end_gap_micros = duration_micros(ended_at.saturating_duration_since(last.presented_at));
    let mut max_gap_micros = start_gap_micros.max(end_gap_micros);
    let mut worst_gap_after_sequence = None;
    let mut worst_gap_timing = None;
    for pair in samples.windows(2) {
        let gap_micros = duration_micros(
            pair[1]
                .presented_at
                .saturating_duration_since(pair[0].presented_at),
        );
        if gap_micros > max_gap_micros {
            max_gap_micros = gap_micros;
            worst_gap_after_sequence = Some(pair[1].sequence);
            worst_gap_timing = pair[1].presentation_timing;
        }
    }
    let changed_count = samples
        .iter()
        .filter(|sample| sample.changed_from_previous == Some(true))
        .count();
    let mut timing_sample_count = 0;
    let mut timing_max_micros = [0; 4];
    let mut backend_timing_sample_count = 0;
    let mut backend_timing_max_micros = [0; 5];
    for sample in &samples {
        let Some(timing) = sample.presentation_timing else {
            continue;
        };
        timing_sample_count += 1;
        timing_max_micros[0] = timing_max_micros[0].max(duration_micros(timing.frame_pacing_wait));
        timing_max_micros[1] =
            timing_max_micros[1].max(duration_micros(timing.vsync_event_queue_delay));
        timing_max_micros[2] =
            timing_max_micros[2].max(duration_micros(timing.window_dispatch_delay));
        timing_max_micros[3] =
            timing_max_micros[3].max(duration_micros(timing.active_present_duration));
        if let Some(backend_timings) = timing.backend_timings {
            backend_timing_sample_count += 1;
            backend_timing_max_micros[0] = backend_timing_max_micros[0]
                .max(duration_micros(backend_timings.acquire_fence_wait));
            backend_timing_max_micros[1] =
                backend_timing_max_micros[1].max(duration_micros(backend_timings.image_acquire));
            backend_timing_max_micros[2] =
                backend_timing_max_micros[2].max(duration_micros(backend_timings.queue_submit));
            backend_timing_max_micros[3] =
                backend_timing_max_micros[3].max(duration_micros(backend_timings.submission_wait));
            backend_timing_max_micros[4] =
                backend_timing_max_micros[4].max(duration_micros(backend_timings.queue_present));
        }
    }

    BlockSampleContinuity {
        sample_count: samples.len(),
        changed_count,
        unchanged_count: samples.len().saturating_sub(changed_count),
        max_gap_micros,
        start_gap_micros,
        end_gap_micros,
        worst_gap_after_sequence,
        worst_gap_timing,
        history_overflowed: history.history_overflowed,
        timing_sample_count,
        timing_max_micros,
        backend_timing_sample_count,
        backend_timing_max_micros,
    }
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn backend_timing_ms(timings: Option<gfx_core::PresentationTimings>) -> [f64; 5] {
    let Some(timings) = timings else {
        return [0.0; 5];
    };
    [
        timings.acquire_fence_wait,
        timings.image_acquire,
        timings.queue_submit,
        timings.submission_wait,
        timings.queue_present,
    ]
    .map(|duration| micros_to_millis(duration_micros(duration) as usize))
}

fn backend_command_timing_ms(timings: Option<gfx_core::PresentationTimings>) -> [f64; 4] {
    let Some(timings) = timings else {
        return [0.0; 4];
    };
    [
        timings.command_encoder_create,
        timings.command_record,
        timings.fence_reset,
        timings.post_present_cleanup,
    ]
    .map(|duration| micros_to_millis(duration_micros(duration) as usize))
}

fn renderer_timing_ms(timing: Option<WindowAnimationPresentationTiming>) -> [f64; 6] {
    let Some(timing) = timing else {
        return [0.0; 6];
    };
    [
        timing.renderer_scene_prepare,
        timing.submission_prepare,
        timing.retained_resource_prepare,
        timing.frame_prepare_upload,
        timing.backend_present,
        timing.renderer_post_present,
    ]
    .map(|duration| micros_to_millis(duration_micros(duration) as usize))
}

fn renderer_substage_timing_ms(timing: WindowAnimationPresentationTiming) -> [f64; 4] {
    [
        timing.draw_step_prepare,
        timing.buffer_upload,
        timing.atlas_upload,
        timing.offscreen_render,
    ]
    .map(|duration| micros_to_millis(duration_micros(duration) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{WindowAnimationSample, WindowAnimationSamples};

    #[test]
    fn checks_each_sample_and_both_block_boundaries() {
        let started_at = Instant::now();
        let samples = WindowAnimationSamples {
            after_sequence: 10,
            latest_sequence: 13,
            history_overflowed: false,
            samples: vec![
                WindowAnimationSample {
                    sequence: 11,
                    presented_at: started_at + Duration::from_millis(4),
                    changed_from_previous: Some(true),
                    presentation_timing: None,
                },
                WindowAnimationSample {
                    sequence: 12,
                    presented_at: started_at + Duration::from_millis(8),
                    changed_from_previous: Some(true),
                    presentation_timing: None,
                },
                WindowAnimationSample {
                    sequence: 13,
                    presented_at: started_at + Duration::from_millis(12),
                    changed_from_previous: Some(true),
                    presentation_timing: None,
                },
            ],
        };

        let continuity = block_sample_continuity(
            Some(&samples),
            started_at,
            started_at + Duration::from_millis(16),
        );
        assert_eq!(continuity.sample_count, 3);
        assert_eq!(continuity.changed_count, 3);
        assert_eq!(continuity.unchanged_count, 0);
        assert_eq!(continuity.start_gap_micros, 4_000);
        assert_eq!(continuity.end_gap_micros, 4_000);
        assert_eq!(continuity.max_gap_micros, 4_000);
        assert!(!continuity.history_overflowed);
    }

    #[test]
    fn associates_the_largest_inter_sample_gap_with_its_presentation_stages() {
        let started_at = Instant::now();
        let timing = gpui::WindowAnimationPresentationTiming {
            frame_pacing_wait: Duration::from_millis(2),
            vsync_event_queue_delay: Duration::from_millis(3),
            window_dispatch_delay: Duration::from_millis(1),
            active_present_duration: Duration::from_millis(4),
            backend_timings: Some(gfx_core::PresentationTimings {
                command_encoder_create: Duration::from_micros(400),
                command_record: Duration::from_millis(2),
                fence_reset: Duration::from_micros(100),
                queue_present: Duration::from_millis(7),
                post_present_cleanup: Duration::from_micros(200),
                ..gfx_core::PresentationTimings::default()
            }),
            ..Default::default()
        };
        let samples = WindowAnimationSamples {
            after_sequence: 40,
            latest_sequence: 42,
            history_overflowed: false,
            samples: vec![
                WindowAnimationSample {
                    sequence: 41,
                    presented_at: started_at + Duration::from_millis(4),
                    changed_from_previous: Some(true),
                    presentation_timing: Some(Default::default()),
                },
                WindowAnimationSample {
                    sequence: 42,
                    presented_at: started_at + Duration::from_millis(12),
                    changed_from_previous: Some(true),
                    presentation_timing: Some(timing),
                },
            ],
        };

        let continuity = block_sample_continuity(
            Some(&samples),
            started_at,
            started_at + Duration::from_millis(16),
        );

        assert_eq!(continuity.max_gap_micros, 8_000);
        assert_eq!(continuity.worst_gap_after_sequence, Some(42));
        assert_eq!(continuity.worst_gap_timing, Some(timing));
        assert_eq!(
            backend_command_timing_ms(timing.backend_timings),
            [0.4, 2.0, 0.1, 0.2]
        );
        assert_eq!(continuity.backend_timing_sample_count, 1);
        assert_eq!(continuity.backend_timing_max_micros[4], 7_000);
        assert_eq!(continuity.backend_timing_sample_count, 1);
        assert_eq!(continuity.backend_timing_max_micros[4], 7_000);
    }

    #[test]
    fn rejects_a_frozen_frame_even_when_the_animation_has_samples() {
        let started_at = Instant::now();
        let samples = WindowAnimationSamples {
            after_sequence: 20,
            latest_sequence: 22,
            history_overflowed: false,
            samples: vec![
                WindowAnimationSample {
                    sequence: 21,
                    presented_at: started_at + Duration::from_millis(4),
                    changed_from_previous: Some(true),
                    presentation_timing: None,
                },
                WindowAnimationSample {
                    sequence: 22,
                    presented_at: started_at + Duration::from_millis(12),
                    changed_from_previous: Some(false),
                    presentation_timing: None,
                },
            ],
        };

        let continuity = block_sample_continuity(
            Some(&samples),
            started_at,
            started_at + Duration::from_millis(16),
        );
        assert_eq!(continuity.sample_count, 2);
        assert_eq!(continuity.changed_count, 1);
        assert_eq!(continuity.unchanged_count, 1);
        assert_eq!(continuity.max_gap_micros, 8_000);
    }

    #[test]
    fn summarizes_windows_stage_timings_only_inside_the_interval() {
        let started_at = Instant::now();
        let timing = gpui::WindowAnimationPresentationTiming {
            frame_pacing_wait: Duration::from_millis(3),
            vsync_event_queue_delay: Duration::from_millis(2),
            window_dispatch_delay: Duration::from_millis(1),
            active_present_duration: Duration::from_millis(4),
            backend_timings: Some(gfx_core::PresentationTimings {
                acquire_fence_wait: Duration::from_millis(5),
                image_acquire: Duration::from_millis(6),
                command_encoder_create: Duration::from_micros(400),
                command_record: Duration::from_micros(500),
                fence_reset: Duration::from_micros(600),
                queue_submit: Duration::from_millis(7),
                submission_wait: Duration::from_millis(8),
                queue_present: Duration::from_millis(9),
                post_present_cleanup: Duration::from_micros(700),
            }),
            ..Default::default()
        };
        let samples = WindowAnimationSamples {
            after_sequence: 30,
            latest_sequence: 32,
            history_overflowed: false,
            samples: vec![
                WindowAnimationSample {
                    sequence: 31,
                    presented_at: started_at + Duration::from_millis(4),
                    changed_from_previous: Some(true),
                    presentation_timing: Some(timing),
                },
                WindowAnimationSample {
                    sequence: 32,
                    presented_at: started_at + Duration::from_millis(20),
                    changed_from_previous: Some(true),
                    presentation_timing: Some(Default::default()),
                },
            ],
        };

        let continuity = block_sample_continuity(
            Some(&samples),
            started_at,
            started_at + Duration::from_millis(8),
        );
        assert_eq!(continuity.timing_sample_count, 1);
        assert_eq!(continuity.timing_max_micros, [3_000, 2_000, 1_000, 4_000]);
        assert_eq!(continuity.backend_timing_sample_count, 1);
        assert_eq!(
            continuity.backend_timing_max_micros,
            [5_000, 6_000, 7_000, 8_000, 9_000]
        );
        assert_eq!(continuity.backend_timing_sample_count, 1);
        assert_eq!(
            continuity.backend_timing_max_micros,
            [5_000, 6_000, 7_000, 8_000, 9_000]
        );
    }
}

fn minimum_samples(sample_rate: f64, duration: Duration) -> u64 {
    expected_samples(sample_rate, duration)
        .saturating_mul(9)
        .div_ceil(10)
        .max(1)
}

fn max_sample_gap(baseline_interval_p50_micros: u64) -> u64 {
    baseline_interval_p50_micros.saturating_mul(18).div_ceil(10)
}

#[allow(clippy::too_many_arguments)]
fn print_interval_report(
    label: &str,
    baseline: &GateBaseline,
    max_sample_gap_micros: u64,
    started_at: Instant,
    ended_at: Instant,
    before_window: WindowSample,
    after_window: WindowSample,
    before_counters: Counters,
    after_counters: Counters,
    before_renders: usize,
    after_renders: usize,
    require_quiet_ui: bool,
) -> bool {
    let samples = window_animation_samples_since(
        before_window.window_id,
        before_window.animation_sampled_present_count,
    );
    let continuity = block_sample_continuity(samples.as_ref(), started_at, ended_at);
    let skipped = after_window
        .skipped_frame_count
        .saturating_sub(before_window.skipped_frame_count);
    let ui_render_delta = after_renders.saturating_sub(before_renders);
    let elapsed = ended_at.saturating_duration_since(started_at);
    let expected = expected_samples(baseline.sample_rate, elapsed);
    let minimum = minimum_samples(baseline.sample_rate, elapsed);
    let passed = continuity.sample_count as u64 >= minimum
        && continuity.changed_count == continuity.sample_count
        && continuity.unchanged_count == 0
        && continuity.max_gap_micros < max_sample_gap_micros
        && !continuity.history_overflowed
        && before_window.active
        && after_window.active
        && before_window.visible
        && after_window.visible
        && !before_window.minimized
        && !after_window.minimized
        && (!require_quiet_ui || ui_render_delta == 0);
    if !after_window.active || !after_window.visible || after_window.minimized {
        eprintln!(
            "{label}: foreground refresh validation unavailable: active={} visible={} minimized={}",
            after_window.active, after_window.visible, after_window.minimized
        );
    }
    let worst_gap_timing = continuity.worst_gap_timing.unwrap_or_default();
    let worst_gap_backend_timing_ms = backend_timing_ms(worst_gap_timing.backend_timings);
    let worst_gap_renderer_timing_ms = renderer_timing_ms(Some(worst_gap_timing));
    let backend_timing_max_ms = continuity
        .backend_timing_max_micros
        .map(|micros| micros_to_millis(micros as usize));
    println!(
        "{label}: {} active_samples={} target={} changed={} unchanged={} in_interval_gap_ms[max/start/end]={:.3}/{:.3}/{:.3} gap_limit_ms={:.3} history_overflowed={} timing_samples={} stage_max_ms[pacing/queue/dispatch/present]={:.3}/{:.3}/{:.3}/{:.3} backend_timing_samples={} backend_stage_max_ms[fence/acquire/submit/wait/present]={:?} worst_gap_after_seq={:?} worst_gap_stage_ms[pacing/queue/dispatch/present]={:.3}/{:.3}/{:.3}/{:.3} worst_gap_renderer_ms[scene/submit/resources/frame/present/post]={:?} worst_gap_backend_ms[fence/acquire/submit/wait/present]={:?} rolling_interval_ms[p50/p95/p99/max]={:.3}/{:.3}/{:.3}/{:.3} skipped={} ui_render_delta={} window_callback_p99_ms={:.3} frame_p99_ms={:.3} global_samples={}",
        if passed { "PASS" } else { "FAIL" },
        continuity.sample_count,
        expected,
        continuity.changed_count,
        continuity.unchanged_count,
        micros_to_millis(continuity.max_gap_micros as usize),
        micros_to_millis(continuity.start_gap_micros as usize),
        micros_to_millis(continuity.end_gap_micros as usize),
        micros_to_millis(max_sample_gap_micros as usize),
        continuity.history_overflowed,
        continuity.timing_sample_count,
        micros_to_millis(continuity.timing_max_micros[0] as usize),
        micros_to_millis(continuity.timing_max_micros[1] as usize),
        micros_to_millis(continuity.timing_max_micros[2] as usize),
        micros_to_millis(continuity.timing_max_micros[3] as usize),
        continuity.backend_timing_sample_count,
        backend_timing_max_ms,
        continuity.worst_gap_after_sequence,
        micros_to_millis(duration_micros(worst_gap_timing.frame_pacing_wait) as usize),
        micros_to_millis(duration_micros(worst_gap_timing.vsync_event_queue_delay) as usize),
        micros_to_millis(duration_micros(worst_gap_timing.window_dispatch_delay) as usize),
        micros_to_millis(duration_micros(worst_gap_timing.active_present_duration) as usize),
        worst_gap_renderer_timing_ms,
        worst_gap_backend_timing_ms,
        micros_to_millis(after_window.animation_sample_interval_p50_micros as usize),
        micros_to_millis(after_window.animation_sample_interval_p95_micros as usize),
        micros_to_millis(after_window.animation_sample_interval_p99_micros as usize),
        micros_to_millis(after_window.animation_sample_interval_max_micros as usize),
        skipped,
        ui_render_delta,
        micros_to_millis(after_window.present_interval_p99_micros as usize),
        micros_to_millis(after_window.frame_duration_p99_micros as usize),
        after_counters
            .animation_samples
            .saturating_sub(before_counters.animation_samples),
    );
    println!(
        "  worst_gap_renderer_substages_ms[draw/buffer/atlas/offscreen]={:?}",
        renderer_substage_timing_ms(worst_gap_timing),
    );
    println!(
        "  worst_gap_backend_command_ms[encoder/record/fence_reset/cleanup]={:?}",
        backend_command_timing_ms(worst_gap_timing.backend_timings),
    );
    let _ = before_window.present_count;
    passed
}

#[derive(Clone, Copy)]
struct Counters {
    presents: usize,
    animation_samples: usize,
}

fn counter_snapshot() -> Counters {
    let metrics = performance_metrics_snapshot();
    Counters {
        presents: metrics.direct_present_count + metrics.retained_present_count,
        animation_samples: metrics.presentation_animation_distinct_sample_count,
    }
}

fn print_snapshot(
    label: &str,
    copies: usize,
    counters: Counters,
    render_count: usize,
    window_id: u64,
) {
    let snapshot = performance_metrics_snapshot();
    let window = window_sample(window_id);
    let native_vsync = if label == "final" {
        window_metrics_snapshot()
            .into_iter()
            .find(|window| window.window_id == window_id)
            .map(|window| window.native_vsync)
    } else {
        None
    };
    println!(
        "{label}: backend={} adapter={:?} copies={} ui_renders={} global_presents={} global_active_sample_changes={} uploads={} animation_uploads={} passes(mask/main/composite)={}/{}/{} blur_frames={} gpu_wait_us={}",
        backend_label(snapshot.renderer_backend),
        snapshot.gpu_adapter_name,
        copies,
        render_count,
        counters.presents,
        counters.animation_samples,
        snapshot.upload_bytes,
        snapshot.animation_upload_bytes,
        snapshot.mask_pass_count,
        snapshot.main_pass_count,
        snapshot.composite_pass_count,
        snapshot.backdrop_blur_frame_count,
        duration_us(snapshot.gpu_submission_wait_time),
    );
    if let Some(window) = window.as_ref() {
        println!(
            "  window={} active={} visible={} minimized={} animation_active_samples={} animation_changed={} animation_unchanged={} animation_interval_ms[p50/p95/p99/max]={:.3}/{:.3}/{:.3}/{:.3} animation_interval_samples={} callback_frame_ms[p50/p95/p99]={:.3}/{:.3}/{:.3} callback_present_ms[p50/p95/p99]={:.3}/{:.3}/{:.3} callback_frame_samples={} callback_present_samples={} callback_presents={} callback_skipped={}",
            window.window_id,
            window.active,
            window.visible,
            window.minimized,
            window.animation_sampled_present_count,
            window.animation_sample_changed_present_count,
            window.animation_sample_unchanged_present_count,
            micros_to_millis(window.animation_sample_interval_p50_micros as usize),
            micros_to_millis(window.animation_sample_interval_p95_micros as usize),
            micros_to_millis(window.animation_sample_interval_p99_micros as usize),
            micros_to_millis(window.animation_sample_interval_max_micros as usize),
            window.animation_sample_interval_sample_count,
            micros_to_millis(window.frame_duration_p50_micros as usize),
            micros_to_millis(window.frame_duration_p95_micros as usize),
            micros_to_millis(window.frame_duration_p99_micros as usize),
            micros_to_millis(window.present_interval_p50_micros as usize),
            micros_to_millis(window.present_interval_p95_micros as usize),
            micros_to_millis(window.present_interval_p99_micros as usize),
            window.frame_duration_sample_count,
            window.present_interval_sample_count,
            window.present_count,
            window.skipped_frame_count,
        );
        let scale_factor = window.scale_factor_milli as f64 / 1000.0;
        let sample_interval_seconds =
            window.animation_sample_interval_p50_micros as f64 / 1_000_000.0;
        let average_physical_px_per_sample = 12.0 * scale_factor * sample_interval_seconds / 1.3;
        let smoothstep_peak_physical_px_per_sample = average_physical_px_per_sample * 1.5;
        let average_frames_per_physical_pixel = if average_physical_px_per_sample > 0.0 {
            1.0 / average_physical_px_per_sample
        } else {
            f64::INFINITY
        };
        println!(
            "  0-12px/1300ms smoothstep quantization estimate at current cadence: scale_factor={scale_factor:.3}x dpi={:.0} logical_px_per_device_px={:.3} avg_device_px_per_sample={average_physical_px_per_sample:.4} smoothstep_peak_device_px_per_sample={smoothstep_peak_physical_px_per_sample:.4} avg_samples_per_device_pixel={average_frames_per_physical_pixel:.1} (layout origin rounds to device pixels; compositor transform remains fractional)",
            scale_factor * 96.0,
            1.0 / scale_factor,
        );
    }
    if label == "final" {
        if let Some(native_vsync) = &native_vsync {
            print_native_vsync_snapshot(native_vsync);
        } else {
            println!("  native_vsync_final_snapshot=unavailable");
        }
    }
    println!(
        "  latest CPU stages ms: build={} layout={} prepaint={} paint={} scene_finish={} backend_draw={} pack={} encode={} upload={}",
        duration_ms(snapshot.frame_build_time),
        duration_ms(snapshot.frame_layout_time),
        duration_ms(snapshot.frame_prepaint_time),
        duration_ms(snapshot.frame_paint_time),
        duration_ms(snapshot.frame_scene_finish_time),
        duration_ms(snapshot.frame_backend_draw_time),
        duration_ms(snapshot.scene_pack_time),
        duration_ms(snapshot.scene_encode_time),
        duration_ms(snapshot.buffer_upload_time),
    );
}

fn print_native_vsync_gate_delta(
    before: Option<&WindowVSyncMetricsSnapshot>,
    after: Option<&WindowVSyncMetricsSnapshot>,
) {
    let (Some(before), Some(after)) = (before, after) else {
        println!("  native_vsync_gate_delta[boundary_counters]=unavailable");
        return;
    };
    println!(
        "  native_vsync_gate_delta[boundary_counters]: wakes={} fallback_wakes={} active_attempts={} preflight_not_ready={} retries={}",
        after.wake_count.saturating_sub(before.wake_count),
        after
            .fallback_wake_count
            .saturating_sub(before.fallback_wake_count),
        after
            .active_presentation_attempt_count
            .saturating_sub(before.active_presentation_attempt_count),
        after
            .active_presentation_preflight_not_ready_count
            .saturating_sub(before.active_presentation_preflight_not_ready_count),
        after
            .active_presentation_retry_count
            .saturating_sub(before.active_presentation_retry_count),
    );
}

fn print_backend_ready_gate_delta_and_final_queue_delay(
    baseline_wake_count: usize,
    final_snapshot: Option<&WindowBackendReadyMetricsSnapshot>,
) {
    let Some(final_snapshot) = final_snapshot else {
        println!("  backend_ready_gate_delta: unavailable");
        println!("  backend_ready_final_snapshot_queue_delay: unavailable");
        return;
    };
    println!(
        "  backend_ready_gate_delta[wake_count]={}",
        final_snapshot
            .wake_count
            .saturating_sub(baseline_wake_count),
    );
    println!(
        "  backend_ready_final_snapshot_recent_queue_delay_us[p50/p95/max]={}/{}/{} samples={}",
        final_snapshot.queue_delay_p50_micros,
        final_snapshot.queue_delay_p95_micros,
        final_snapshot.queue_delay_max_micros,
        final_snapshot.queue_delay_sample_count,
    );
}

fn print_native_vsync_snapshot(snapshot: &WindowVSyncMetricsSnapshot) {
    let refresh_rate_p50_hz = if snapshot.refresh_period_p50_micros == 0 {
        0.0
    } else {
        1_000_000.0 / snapshot.refresh_period_p50_micros as f64
    };
    println!(
        "  native_vsync_totals: wakes={} fallback_wakes={} active_attempts={} preflight_not_ready={} retries={}",
        snapshot.wake_count,
        snapshot.fallback_wake_count,
        snapshot.active_presentation_attempt_count,
        snapshot.active_presentation_preflight_not_ready_count,
        snapshot.active_presentation_retry_count,
    );
    println!(
        "  native_vsync_recent_ms: wake_interval[p50/p95/p99/max]={:.3}/{:.3}/{:.3}/{:.3} samples={} dwm_qpc_refresh_period[p50/p95/p99/max]={:.3}/{:.3}/{:.3}/{:.3} samples={} refresh_rate_from_qpc_p50_hz={refresh_rate_p50_hz:.3} dwm_composition_period[p50/p95/p99/max]={:.3}/{:.3}/{:.3}/{:.3} samples={}",
        micros_to_millis(snapshot.wake_interval_p50_micros),
        micros_to_millis(snapshot.wake_interval_p95_micros),
        micros_to_millis(snapshot.wake_interval_p99_micros),
        micros_to_millis(snapshot.wake_interval_max_micros),
        snapshot.wake_interval_sample_count,
        micros_to_millis(snapshot.refresh_period_p50_micros),
        micros_to_millis(snapshot.refresh_period_p95_micros),
        micros_to_millis(snapshot.refresh_period_p99_micros),
        micros_to_millis(snapshot.refresh_period_max_micros),
        snapshot.refresh_period_sample_count,
        micros_to_millis(snapshot.composition_period_p50_micros),
        micros_to_millis(snapshot.composition_period_p95_micros),
        micros_to_millis(snapshot.composition_period_p99_micros),
        micros_to_millis(snapshot.composition_period_max_micros),
        snapshot.composition_period_sample_count,
    );
}
