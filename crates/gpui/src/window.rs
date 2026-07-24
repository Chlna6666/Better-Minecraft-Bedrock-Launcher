#[cfg(any(feature = "inspector", debug_assertions))]
use crate::Inspector;
use crate::{
    Action, AnimatedFrame, AnyDrag, AnyElement, AnyImageCache, AnyTooltip, AnyView, App,
    AppContext, Arena, Asset, AsyncApp, AsyncWindowContext, AtlasTile, AvailableSpace,
    BackdropBlurStyle, Background, BorderStyle, Bounds, BoxShadow, Capslock, Context, Corners,
    CursorStyle, DevicePixels, DirtyRegion, DispatchActionListener, DispatchNodeId, DispatchTree,
    DisplayId, Edges, Effect, Entity, EntityId, EventEmitter, FileDropEvent, FontId,
    FrameRenderPlan, FrameVisualEffectQuality, Global, GlobalElementId, GlyphId, GpuMesh3d,
    GpuMesh3dDrawParameters, GpuSpecs, GpuiMemoryTrimLevel, Hsla, ImagePipelineConfig,
    InputHandler, IsZero, KeyBinding, KeyContext, KeyDownEvent, KeyEvent, Keystroke,
    KeystrokeEvent, LayoutFrameMetrics, LayoutId, LineLayoutFrameMetrics, LineLayoutIndex,
    Modifiers, ModifiersChangedEvent, MonochromeSprite, MonochromeSpriteSampling, MouseButton,
    MouseDownEvent, MouseEvent, MouseMoveEvent, MouseUpEvent, PartialPresentMode, Path, Pixels,
    PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow, Point,
    PolychromeSprite, Quad, Render, RenderGlyphParams, RenderImage, RenderImageParams,
    RenderImagePixelFormat, RenderSvgParams, Replay, RequestFrameOptions,
    RetainedResourceTrimPolicy, SMOOTH_SVG_SCALE_FACTOR, SUBPIXEL_VARIANTS_Y, ScaledPixels, Scene,
    SceneFrameMetrics, Shadow, SharedString, Size, StrikethroughStyle, Style, SubscriberSet,
    Subscription, SystemWindowTab, SystemWindowTabController, TaffyLayoutEngine, Task, TextStyle,
    TextStyleRefinement, TransformationMatrix, Underline, UnderlineStyle, WindowFrameDisposition,
    WindowTextSystem,
    animation::{
        AnimationDriver, AnimationEngine, AnimationGroupId, AnimationGroupSample,
        AnimationParallel, AnimationSequence, AnimationStagger, merge_requested_drivers,
    },
    point,
    prelude::*,
    px, record_coalesced_refresh, record_dirty_region_metrics, record_frame_decision,
    record_frame_retained_capacity, record_image_drop, record_inactive_present_skip,
    record_layout_cache_metrics, record_layout_frame_metrics, record_retained_frame_skip,
    record_scene_frame_metrics, record_skipped_pointer_frame, record_window_frame_disposition,
    record_window_layout_recompute, rems, size, transparent_black,
};
use anyhow::{Context as _, Result, anyhow};
use collections::{FxHashMap, FxHashSet};
use derive_more::{Deref, DerefMut};
use futures::FutureExt;
use itertools::FoldWhile::{Continue, Done};
use itertools::Itertools;
use parking_lot::RwLock;
use refineable::Refineable;
use slotmap::SlotMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp,
    fmt::{Debug, Display},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::{DerefMut, Range},
    rc::Rc,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use util::post_inc;
use util::{ResultExt, measure};
use uuid::Uuid;
use winit::raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};

mod action_bindings;
mod content_mask;
mod dispatch;
mod draw;
mod draw_reuse;
mod element_context;
mod element_id;
mod element_state;
mod focus;
mod frame;
mod frame_lifecycle;
mod frame_scheduling;
mod handle;
mod hitbox;
mod input;
#[cfg(any(feature = "inspector", debug_assertions))]
mod inspector;
mod key_dispatch;
mod layout;
mod lifecycle;
mod observers;
mod open;
mod options;
mod paint;
mod paint_resources;
mod platform_actions;
mod platform_events;
mod prompts;
mod properties;
mod state;
mod tab_stop;
#[cfg(test)]
mod tests;
mod titlebar;
mod window_id;

pub use content_mask::ContentMask;
use dispatch::{log_timed_gpui_event, platform_input_name};
pub use element_id::ElementId;
pub(crate) use focus::{AnyWindowFocusListener, ELEMENT_ARENA, FocusMap, WindowFocusEvent};
pub use focus::{
    ArenaClearNeeded, DismissEvent, FocusHandle, FocusId, FocusOutEvent, Focusable, ManagedView,
    WeakFocusHandle,
};
use frame::{
    DIRTY_REGION_FULL_REDRAW_RATIO, WINDOW_LIGHT_TRIM_IDLE_FRAMES, WINDOW_STRONG_TRIM_IDLE_FRAMES,
};
pub(crate) use frame::{DeferredDraw, Frame, PaintIndex, PrepaintStateIndex, RetainedSceneSegment};
#[cfg(test)]
use frame_lifecycle::FrameCompletion;
use frame_lifecycle::{DIRTY_FRAME_BACKPRESSURE_BUDGET, FrameWatchdog, WindowFrameThrottle};
pub use handle::{AnyWindowHandle, WindowHandle};
pub(crate) use hitbox::{
    CursorStyleRequest, HitTest, MouseListener, TooltipBounds, TooltipRequest,
};
pub use hitbox::{Hitbox, HitboxBehavior, HitboxId, TooltipId, WindowControlArea};
pub use lifecycle::DispatchPhase;
pub(crate) use lifecycle::{
    AnyObserver, FrameCallback, WindowInvalidator, ignore_window_not_found,
};
pub(crate) use open::DEFAULT_WINDOW_SIZE;
pub use options::*;
pub use paint::{PaintQuad, fill, outline, quad};
pub use paint_resources::{ImagePaintProgress, ImagePaintRequest};
pub use prompts::*;
pub use state::Window;
use state::{
    AnimatedImageSlotKey, DirtyFrameDiagnostics, FrameGenerationStats, ImagePaintTileCacheKey,
    LayoutCacheFrameMetrics, ModifierState, PendingInput,
};
pub(crate) use state::{DispatchEventResult, DrawPhase, ElementStateBox};
pub(crate) use tab_stop::*;
pub use titlebar::TitlebarGestureState;
use titlebar::{
    glyph_device_origin, resize_edge_cursor_style, resize_edge_hit_test,
    svg_paint_bounds_for_requested_bounds, svg_raster_size_for_paint_bounds,
};
pub use window_id::WindowId;
