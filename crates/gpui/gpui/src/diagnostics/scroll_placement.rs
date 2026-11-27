//! Placement-only scroll admission outcomes. One relaxed counter per attempted transaction.
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[repr(usize)]
pub(crate) enum Outcome {
    Committed,
    Invalidation,
    FrameState,
    Viewport,
    DirtyViews,
    InteractiveOverlay,
    Animation,
    Debug,
    UnsupportedBackend,
    MissingPaint,
    MissingCapture,
    MultipleCaptures,
    CaptureDependencies,
    MountedSources,
    OffsetRange,
    RetentionRecord,
    NestedRetention,
    InputRecords,
    Hover,
    Marker,
    PreviousTranslation,
    PreviousPlacement,
    PreviousMarker,
    FractionalTranslation,
    Coverage,
    PlacementGeometry,
    Damage,
    ScenePublication,
    CommitRace,
}

const OUTCOMES: [Outcome; 29] = [
    Outcome::Committed,
    Outcome::Invalidation,
    Outcome::FrameState,
    Outcome::Viewport,
    Outcome::DirtyViews,
    Outcome::InteractiveOverlay,
    Outcome::Animation,
    Outcome::Debug,
    Outcome::UnsupportedBackend,
    Outcome::MissingPaint,
    Outcome::MissingCapture,
    Outcome::MultipleCaptures,
    Outcome::CaptureDependencies,
    Outcome::MountedSources,
    Outcome::OffsetRange,
    Outcome::RetentionRecord,
    Outcome::NestedRetention,
    Outcome::InputRecords,
    Outcome::Hover,
    Outcome::Marker,
    Outcome::PreviousTranslation,
    Outcome::PreviousPlacement,
    Outcome::PreviousMarker,
    Outcome::FractionalTranslation,
    Outcome::Coverage,
    Outcome::PlacementGeometry,
    Outcome::Damage,
    Outcome::ScenePublication,
    Outcome::CommitRace,
];

static COUNTS: [AtomicU64; OUTCOMES.len()] =
    [const { AtomicU64::new(0) }; OUTCOMES.len()];

pub(crate) fn record(outcome: Outcome) {
    COUNTS[outcome as usize].fetch_add(1, Ordering::Relaxed);
}

/// Returns process-lifetime placement outcomes as a JSON-serializable map.
/// Counts include warmup and all windows. Frames with no pending placement
/// epoch are not attempts. Each attempt contributes to exactly one outcome.
pub fn snapshot() -> std::collections::BTreeMap<&'static str, u64> {
    OUTCOMES.iter().map(|&outcome| {
        let name = match outcome {
            Outcome::Committed => "committed",
            Outcome::Invalidation => "invalidation",
            Outcome::FrameState => "frame-state",
            Outcome::Viewport => "viewport",
            Outcome::DirtyViews => "dirty-views",
            Outcome::InteractiveOverlay => "interactive-overlay",
            Outcome::Animation => "animation",
            Outcome::Debug => "debug",
            Outcome::UnsupportedBackend => "unsupported-backend",
            Outcome::MissingPaint => "missing-paint",
            Outcome::MissingCapture => "missing-capture",
            Outcome::MultipleCaptures => "multiple-captures",
            Outcome::CaptureDependencies => "capture-dependencies",
            Outcome::MountedSources => "mounted-sources",
            Outcome::OffsetRange => "offset-range",
            Outcome::RetentionRecord => "retention-record",
            Outcome::NestedRetention => "nested-retention",
            Outcome::InputRecords => "input-records",
            Outcome::Hover => "hover",
            Outcome::Marker => "marker",
            Outcome::PreviousTranslation => "previous-translation",
            Outcome::PreviousPlacement => "previous-placement",
            Outcome::PreviousMarker => "previous-marker",
            Outcome::FractionalTranslation => "fractional-translation",
            Outcome::Coverage => "coverage",
            Outcome::PlacementGeometry => "placement-geometry",
            Outcome::Damage => "damage",
            Outcome::ScenePublication => "scene-publication",
            Outcome::CommitRace => "commit-race",
        };
        (name, COUNTS[outcome as usize].load(Ordering::Relaxed))
    }).collect()
}
