#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub use bmcbl_plugin_macros::{bmcbl_plugin, plugin_metadata};

#[cfg(feature = "pack")]
pub mod pack;

pub const API_VERSION: &str = "0.4";
pub const HOST_MODULE: &str = "bmcbl";
pub const HOST_CALL_NAME: &str = "bmcbl_host_call";
pub const DEFAULT_HOST_BUFFER_CAPACITY: usize = 256;
pub const MAX_HOST_BUFFER_CAPACITY: usize = 1024 * 1024;

const OP_LOG: i32 = 0;
const OP_SHOW_TOAST: i32 = 1;
const OP_NAVIGATE: i32 = 2;
const OP_OPEN_WINDOW: i32 = 3;
const OP_CLOSE_WINDOW: i32 = 4;
const OP_EMIT_EVENT: i32 = 5;
const OP_INVALIDATE: i32 = 6;
const OP_CURRENT_LOCALE: i32 = 7;
const OP_TRANSLATE: i32 = 8;
const OP_READ_CONFIG: i32 = 9;
const OP_HTTP_GET_TEXT: i32 = 10;
const OP_WRITE_CLIPBOARD_TEXT: i32 = 11;
const OP_CURRENT_UNIX_MS: i32 = 12;
const OP_OPEN_EXTERNAL_URL: i32 = 13;
const OP_SESSION_GET: i32 = 14;
const OP_SESSION_SET: i32 = 15;
const OP_THEME_SNAPSHOT: i32 = 16;
const OP_OPEN_MODAL: i32 = 17;
const OP_READ_CLIPBOARD_TEXT: i32 = 18;
const OP_READ_RESOURCE_TEXT: i32 = 19;
const OP_READ_RESOURCE_BYTES: i32 = 20;
const OP_STORAGE_GET: i32 = 21;
const OP_STORAGE_SET: i32 = 22;
const OP_STORAGE_DELETE: i32 = 23;
const OP_STORAGE_LIST: i32 = 24;
const OP_WRITE_CONFIG: i32 = 25;
const OP_CREATE_TASK: i32 = 26;
const OP_UPDATE_TASK: i32 = 27;
const OP_FINISH_TASK: i32 = 28;
const OP_APP_INFO: i32 = 29;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum HostOp {
    Log = OP_LOG,
    ShowToast = OP_SHOW_TOAST,
    Navigate = OP_NAVIGATE,
    OpenWindow = OP_OPEN_WINDOW,
    CloseWindow = OP_CLOSE_WINDOW,
    EmitEvent = OP_EMIT_EVENT,
    Invalidate = OP_INVALIDATE,
    CurrentLocale = OP_CURRENT_LOCALE,
    Translate = OP_TRANSLATE,
    ReadConfig = OP_READ_CONFIG,
    HttpGetText = OP_HTTP_GET_TEXT,
    WriteClipboardText = OP_WRITE_CLIPBOARD_TEXT,
    CurrentUnixMs = OP_CURRENT_UNIX_MS,
    OpenExternalUrl = OP_OPEN_EXTERNAL_URL,
    SessionGet = OP_SESSION_GET,
    SessionSet = OP_SESSION_SET,
    ThemeSnapshot = OP_THEME_SNAPSHOT,
    OpenModal = OP_OPEN_MODAL,
    ReadClipboardText = OP_READ_CLIPBOARD_TEXT,
    ReadResourceText = OP_READ_RESOURCE_TEXT,
    ReadResourceBytes = OP_READ_RESOURCE_BYTES,
    StorageGet = OP_STORAGE_GET,
    StorageSet = OP_STORAGE_SET,
    StorageDelete = OP_STORAGE_DELETE,
    StorageList = OP_STORAGE_LIST,
    WriteConfig = OP_WRITE_CONFIG,
    CreateTask = OP_CREATE_TASK,
    UpdateTask = OP_UPDATE_TASK,
    FinishTask = OP_FINISH_TASK,
    AppInfo = OP_APP_INFO,
}

impl HostOp {
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginContext {
    pub plugin_id: String,
    pub api_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

impl PluginError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new("denied", message)
    }

    #[must_use]
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid-input", message)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not-found", message)
    }

    #[must_use]
    pub fn host(message: impl Into<String>) -> Self {
        Self::new("host", message)
    }

    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new("timeout", message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HostError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AbiResult<T> {
    Ok(T),
    Err(PluginError),
}

pub type PluginResult<T> = Result<T, PluginError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayoutDirection {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ThemeToken {
    PrimaryText,
    SecondaryText,
    MutedText,
    Accent,
    Surface,
    Border,
    Danger,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThemeColor {
    pub h: f32,
    pub s: f32,
    pub l: f32,
    pub a: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThemeSnapshot {
    pub mode: ThemeMode,
    pub dark_factor: f32,
    pub accent: ThemeColor,
    pub primary_text: ThemeColor,
    pub secondary_text: ThemeColor,
    pub muted_text: ThemeColor,
    pub surface: ThemeColor,
    pub surface_hover: ThemeColor,
    pub border: ThemeColor,
    pub danger: ThemeColor,
}

impl ThemeSnapshot {
    #[must_use]
    pub const fn light_default() -> Self {
        Self {
            mode: ThemeMode::Light,
            dark_factor: 0.0,
            accent: ThemeColor {
                h: 0.60,
                s: 0.88,
                l: 0.60,
                a: 1.0,
            },
            primary_text: ThemeColor {
                h: 0.61,
                s: 0.30,
                l: 0.20,
                a: 1.0,
            },
            secondary_text: ThemeColor {
                h: 0.60,
                s: 0.16,
                l: 0.45,
                a: 1.0,
            },
            muted_text: ThemeColor {
                h: 0.59,
                s: 0.14,
                l: 0.61,
                a: 1.0,
            },
            surface: ThemeColor {
                h: 0.58,
                s: 0.24,
                l: 0.96,
                a: 0.86,
            },
            surface_hover: ThemeColor {
                h: 0.58,
                s: 0.18,
                l: 0.93,
                a: 0.90,
            },
            border: ThemeColor {
                h: 0.60,
                s: 0.20,
                l: 0.84,
                a: 0.72,
            },
            danger: ThemeColor {
                h: 0.06,
                s: 0.93,
                l: 0.53,
                a: 1.0,
            },
        }
    }

    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self.mode, ThemeMode::Dark)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TextSizeToken {
    Small,
    Body,
    Title,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ImageFit {
    Cover,
    Contain,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum InjectionSlot {
    MainRootOverlay,
    PageHeader,
    PageBody,
    HomeSidebar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ShutdownReason {
    Reload,
    Unload,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct I18nArg {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HttpCacheState {
    Loading,
    Fresh,
    Stale,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HttpTextResponse {
    pub state: HttpCacheState,
    pub body: Option<String>,
    pub error: Option<String>,
    pub fetched_at_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionEntry {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StorageEntry {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskCreateRequest {
    pub task_id: Option<String>,
    pub title: String,
    pub detail: Option<String>,
    pub stage: String,
    pub total: Option<u64>,
    pub supports_pause: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskUpdateRequest {
    pub task_id: String,
    pub stage: Option<String>,
    pub total: Option<u64>,
    pub done_delta: u64,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskFinishRequest {
    pub task_id: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub build_info: String,
    pub api_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RouteTarget {
    pub plugin_id: Option<String>,
    pub page_id: Option<String>,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WindowRequest {
    pub title: String,
    pub plugin_id: String,
    pub page_id: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModalRequest {
    pub title: String,
    pub plugin_id: String,
    pub page_id: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PageRenderRequest {
    pub plugin_id: String,
    pub page_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InjectionRequest {
    pub slot: InjectionSlot,
    pub page: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NavigationEntry {
    pub label: String,
    pub icon: Option<String>,
    pub order: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PageRegistration {
    pub page_id: String,
    pub title: String,
    pub navigation: Option<NavigationEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InjectionRegistration {
    pub slot: InjectionSlot,
    pub page: Option<String>,
    pub priority: i32,
    pub layout: Option<InjectionLayout>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CompactBehavior {
    None,
    Scroll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InjectionLayout {
    pub preferred_width: Option<u16>,
    pub min_width: Option<u16>,
    pub max_width: Option<u16>,
    pub max_height: Option<u16>,
    pub priority: i32,
    pub compact_behavior: CompactBehavior,
}

impl InjectionLayout {
    #[must_use]
    pub const fn sidebar() -> Self {
        Self {
            preferred_width: None,
            min_width: None,
            max_width: None,
            max_height: None,
            priority: 0,
            compact_behavior: CompactBehavior::None,
        }
    }

    #[must_use]
    pub const fn width(mut self, width: u16) -> Self {
        self.preferred_width = Some(width);
        self
    }

    #[must_use]
    pub const fn min_width(mut self, width: u16) -> Self {
        self.min_width = Some(width);
        self
    }

    #[must_use]
    pub const fn max_width(mut self, width: u16) -> Self {
        self.max_width = Some(width);
        self
    }

    #[must_use]
    pub const fn max_height(mut self, height: u16) -> Self {
        self.max_height = Some(height);
        self
    }

    #[must_use]
    pub const fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub const fn compact_behavior(mut self, behavior: CompactBehavior) -> Self {
        self.compact_behavior = behavior;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventSubscription {
    pub event: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Registration {
    Page(PageRegistration),
    Injection(InjectionRegistration),
    Subscription(EventSubscription),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RouteChangedEvent {
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActionEvent {
    pub action_id: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GlobalEvent {
    pub name: String,
    pub payload: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HostEventKind {
    RouteChanged(RouteChangedEvent),
    Action(ActionEvent),
    Global(GlobalEvent),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HostEvent {
    pub plugin_id: Option<String>,
    pub page_id: Option<String>,
    pub kind: HostEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InvalidateTarget {
    All,
    Page(String),
    Injection(InjectionRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViewStyle {
    pub direction: LayoutDirection,
    pub gap: u16,
    pub padding: u16,
    pub align: Align,
    pub color: Option<ThemeToken>,
    pub background: Option<ThemeToken>,
    pub text_size: TextSizeToken,
    pub emphasis: bool,
    pub full_width: bool,
    pub corner_radius: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContainerNode {
    pub style: ViewStyle,
    pub children: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextNode {
    pub text: String,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ButtonNode {
    pub label: String,
    pub action_id: String,
    pub action_value: Option<String>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputNode {
    pub value: String,
    pub placeholder: String,
    pub action_id: String,
    pub action_value: Option<String>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CheckboxNode {
    pub label: String,
    pub checked: bool,
    pub action_id: String,
    pub action_value: Option<String>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToggleNode {
    pub label: String,
    pub enabled: bool,
    pub action_id: String,
    pub action_value: Option<String>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectNode {
    pub label: String,
    pub action_id: String,
    pub options: Vec<SelectOption>,
    pub selected: Option<String>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProgressNode {
    pub label: String,
    pub value: u64,
    pub total: Option<u64>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LinkNode {
    pub label: String,
    pub url: String,
    pub tooltip: Option<String>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ListNode {
    pub items: Vec<u32>,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BadgeNode {
    pub label: String,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IconNode {
    pub name: String,
    pub style: ViewStyle,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImageNode {
    pub src: String,
    pub alt: String,
    pub caption: String,
    pub placeholder: String,
    pub fallback: String,
    pub style: ViewStyle,
    pub height: Option<u16>,
    pub min_height: Option<u16>,
    pub max_height: Option<u16>,
    pub aspect_ratio_x: Option<u16>,
    pub aspect_ratio_y: Option<u16>,
    pub corner_radius: Option<u16>,
    pub fit: ImageFit,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImageOptions {
    pub caption: String,
    pub placeholder: String,
    pub fallback: String,
    pub height: Option<u16>,
    pub min_height: Option<u16>,
    pub max_height: Option<u16>,
    pub aspect_ratio_x: Option<u16>,
    pub aspect_ratio_y: Option<u16>,
    pub corner_radius: Option<u16>,
    pub fit: ImageFit,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            caption: String::new(),
            placeholder: String::new(),
            fallback: String::new(),
            height: None,
            min_height: None,
            max_height: None,
            aspect_ratio_x: None,
            aspect_ratio_y: None,
            corner_radius: None,
            fit: ImageFit::Cover,
        }
    }
}

impl ImageOptions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = caption.into();
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    #[must_use]
    pub fn fallback(mut self, fallback: impl Into<String>) -> Self {
        self.fallback = fallback.into();
        self
    }

    #[must_use]
    pub fn height(mut self, height: u16) -> Self {
        self.height = Some(height);
        self
    }

    #[must_use]
    pub fn min_height(mut self, min_height: u16) -> Self {
        self.min_height = Some(min_height);
        self
    }

    #[must_use]
    pub fn max_height(mut self, max_height: u16) -> Self {
        self.max_height = Some(max_height);
        self
    }

    #[must_use]
    pub fn aspect_ratio(mut self, width: u16, height: u16) -> Self {
        self.aspect_ratio_x = Some(width);
        self.aspect_ratio_y = Some(height);
        self
    }

    #[must_use]
    pub fn corner_radius(mut self, corner_radius: u16) -> Self {
        self.corner_radius = Some(corner_radius);
        self
    }

    #[must_use]
    pub fn fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpacerNode {
    pub size: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ViewNode {
    Container(ContainerNode),
    Text(TextNode),
    Button(ButtonNode),
    Input(InputNode),
    Checkbox(CheckboxNode),
    Toggle(ToggleNode),
    Select(SelectNode),
    Progress(ProgressNode),
    Link(LinkNode),
    ItemList(ListNode),
    Separator,
    Badge(BadgeNode),
    Icon(IconNode),
    Image(ImageNode),
    Spacer(SpacerNode),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViewTree {
    pub root: u32,
    pub nodes: Vec<ViewNode>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HostRequest {
    Log {
        level: LogLevel,
        message: String,
    },
    ShowToast {
        kind: ToastKind,
        message: String,
    },
    Navigate {
        target: RouteTarget,
    },
    OpenWindow {
        request: WindowRequest,
    },
    CloseWindow {
        window_id: u64,
    },
    EmitEvent {
        name: String,
        payload: String,
    },
    Invalidate {
        target: InvalidateTarget,
    },
    CurrentLocale,
    Translate {
        key: String,
        args: Vec<I18nArg>,
    },
    ReadConfig,
    HttpGetText {
        url: String,
        ttl_seconds: u32,
        max_bytes: u32,
    },
    WriteClipboardText {
        text: String,
    },
    CurrentUnixMs,
    OpenExternalUrl {
        url: String,
    },
    SessionGet {
        key: String,
    },
    SessionSet {
        key: String,
        value: Option<String>,
    },
    ThemeSnapshot,
    OpenModal {
        request: ModalRequest,
    },
    ReadClipboardText,
    ReadResourceText {
        path: String,
    },
    ReadResourceBytes {
        path: String,
    },
    StorageGet {
        key: String,
    },
    StorageSet {
        key: String,
        value: String,
    },
    StorageDelete {
        key: String,
    },
    StorageList {
        prefix: Option<String>,
    },
    WriteConfig {
        text: String,
    },
    CreateTask {
        request: TaskCreateRequest,
    },
    UpdateTask {
        request: TaskUpdateRequest,
    },
    FinishTask {
        request: TaskFinishRequest,
    },
    AppInfo,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HostResponse {
    Unit,
    WindowId(u64),
    String(String),
    U64(u64),
    HttpTextResponse {
        state: HttpCacheState,
        body: Option<String>,
        error: Option<String>,
        fetched_at_unix_ms: Option<u64>,
    },
    SessionValue(Option<String>),
    Bytes(Vec<u8>),
    StringList(Vec<String>),
    TaskId(String),
    AppInfo(AppInfo),
    ThemeSnapshot(ThemeSnapshot),
}

pub trait Plugin {
    fn init(context: PluginContext) -> PluginResult<Vec<Registration>>;

    fn handle_event(_event: HostEvent) -> PluginResult<()> {
        Ok(())
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree>;

    fn render_injection(_request: InjectionRequest) -> PluginResult<Option<ViewTree>> {
        Ok(None)
    }

    fn shutdown(_reason: ShutdownReason) -> PluginResult<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginMetadata {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub authors: &'static [&'static str],
    pub description: &'static str,
    pub website: &'static str,
    pub license: &'static str,
    pub tags: &'static [&'static str],
    pub capabilities: &'static [&'static str],
}

impl PluginMetadata {
    #[must_use]
    pub fn authors_display(self) -> String {
        self.authors.join(", ")
    }

    #[must_use]
    pub fn capabilities_display(self) -> String {
        self.capabilities.join(", ")
    }

    #[must_use]
    pub fn window(self, page_id: impl Into<String>) -> Window {
        Window::new(page_id)
    }

    pub fn open_window(self, page_id: impl Into<String>) -> PluginResult<u64> {
        self.window(page_id).open(self)
    }

    #[must_use]
    pub fn modal(self, page_id: impl Into<String>) -> Modal {
        Modal::new(page_id)
    }

    pub fn open_modal(self, page_id: impl Into<String>) -> PluginResult<()> {
        self.modal(page_id).open(self)
    }

    pub fn navigate_page(self, page_id: impl Into<String>) -> PluginResult<()> {
        navigate_plugin(self.id, page_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginAction {
    String(&'static str),
}

impl PluginAction {
    #[must_use]
    pub const fn new(action_id: &'static str) -> Self {
        Self::String(action_id)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String(action_id) => action_id,
        }
    }
}

impl From<PluginAction> for String {
    fn from(action: PluginAction) -> Self {
        action.as_str().to_string()
    }
}

impl HostEvent {
    #[must_use]
    pub fn action_id(&self) -> Option<&str> {
        match &self.kind {
            HostEventKind::Action(action) => Some(action.action_id.as_str()),
            HostEventKind::RouteChanged(_) | HostEventKind::Global(_) => None,
        }
    }

    #[must_use]
    pub fn action_is(&self, action_id: impl AsRef<str>) -> bool {
        self.action_id()
            .is_some_and(|current| current == action_id.as_ref())
    }

    #[must_use]
    pub fn route_path(&self) -> Option<&str> {
        match &self.kind {
            HostEventKind::RouteChanged(route) => Some(route.path.as_str()),
            HostEventKind::Action(_) | HostEventKind::Global(_) => None,
        }
    }

    #[must_use]
    pub fn global_event(&self) -> Option<(&str, &str)> {
        match &self.kind {
            HostEventKind::Global(event) => Some((event.name.as_str(), event.payload.as_str())),
            HostEventKind::Action(_) | HostEventKind::RouteChanged(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Registrations {
    items: Vec<Registration>,
}

impl Registrations {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn page(mut self, page: Page) -> Self {
        self.items.push(Registration::Page(page.finish()));
        self
    }

    #[must_use]
    pub fn injection(mut self, injection: Injection) -> Self {
        self.items.push(Registration::Injection(injection.finish()));
        self
    }

    #[must_use]
    pub fn subscribe(mut self, event: impl Into<String>) -> Self {
        self.items
            .push(Registration::Subscription(EventSubscription {
                event: event.into(),
            }));
        self
    }

    #[must_use]
    pub fn register(mut self, registration: impl IntoRegistration) -> Self {
        self.items.push(registration.into_registration());
        self
    }

    #[must_use]
    pub fn extend(mut self, registrations: impl IntoIterator<Item = Registration>) -> Self {
        self.items.extend(registrations);
        self
    }

    #[must_use]
    pub fn finish(self) -> Vec<Registration> {
        self.items
    }
}

pub trait IntoRegistration {
    fn into_registration(self) -> Registration;
}

impl IntoRegistration for Registration {
    fn into_registration(self) -> Registration {
        self
    }
}

impl IntoRegistration for Page {
    fn into_registration(self) -> Registration {
        Registration::Page(self.finish())
    }
}

impl IntoRegistration for Injection {
    fn into_registration(self) -> Registration {
        Registration::Injection(self.finish())
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    page_id: String,
    title: String,
    navigation: Option<NavigationEntry>,
}

impl Page {
    #[must_use]
    pub fn new(page_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            page_id: page_id.into(),
            title: title.into(),
            navigation: None,
        }
    }

    #[must_use]
    pub fn nav(mut self, navigation: Nav) -> Self {
        self.navigation = Some(navigation.finish());
        self
    }

    #[must_use]
    pub fn finish(self) -> PageRegistration {
        PageRegistration {
            page_id: self.page_id,
            title: self.title,
            navigation: self.navigation,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Nav {
    label: String,
    icon: Option<String>,
    order: i32,
}

impl Nav {
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            order: 0,
        }
    }

    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    #[must_use]
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    #[must_use]
    pub fn finish(self) -> NavigationEntry {
        NavigationEntry {
            label: self.label,
            icon: self.icon,
            order: self.order,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Injection {
    slot: InjectionSlot,
    page: Option<String>,
    priority: i32,
    layout: Option<InjectionLayout>,
}

impl Injection {
    #[must_use]
    pub fn new(slot: InjectionSlot) -> Self {
        Self {
            slot,
            page: None,
            priority: 0,
            layout: None,
        }
    }

    #[must_use]
    pub fn page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    #[must_use]
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub fn layout(mut self, layout: InjectionLayout) -> Self {
        self.layout = Some(layout);
        self
    }

    #[must_use]
    pub fn finish(self) -> InjectionRegistration {
        InjectionRegistration {
            slot: self.slot,
            page: self.page,
            priority: self.priority,
            layout: self.layout,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Window {
    page_id: String,
    title: Option<String>,
    width: u32,
    height: u32,
    resizable: bool,
}

impl Window {
    #[must_use]
    pub fn new(page_id: impl Into<String>) -> Self {
        Self {
            page_id: page_id.into(),
            title: None,
            width: 780,
            height: 520,
            resizable: true,
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    #[must_use]
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn open(self, plugin: PluginMetadata) -> PluginResult<u64> {
        open_window(&WindowRequest {
            title: self.title.unwrap_or_else(|| plugin.name.to_string()),
            plugin_id: plugin.id.to_string(),
            page_id: self.page_id,
            width: self.width,
            height: self.height,
            resizable: self.resizable,
        })
    }
}

pub struct Modal {
    page_id: String,
    title: Option<String>,
    width: u32,
    height: u32,
}

impl Modal {
    #[must_use]
    pub fn new(page_id: impl Into<String>) -> Self {
        Self {
            page_id: page_id.into(),
            title: None,
            width: 760,
            height: 560,
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn open(self, plugin: PluginMetadata) -> PluginResult<()> {
        open_modal(&ModalRequest {
            title: self.title.unwrap_or_else(|| plugin.name.to_string()),
            plugin_id: plugin.id.to_string(),
            page_id: self.page_id,
            width: self.width,
            height: self.height,
        })
    }
}

#[derive(Clone, Debug)]
pub struct View {
    node: ViewNode,
    children: Vec<View>,
}

impl View {
    #[must_use]
    pub fn node(node: ViewNode) -> Self {
        Self {
            node,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn column() -> Container {
        Container::new(LayoutDirection::Column)
    }

    #[must_use]
    pub fn row() -> Container {
        Container::new(LayoutDirection::Row)
    }

    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::node(crate::text(value))
    }

    #[must_use]
    pub fn title(text: impl Into<String>) -> Self {
        Self::node(title(text))
    }

    #[must_use]
    pub fn badge(label: impl Into<String>) -> Self {
        Self::node(badge(label))
    }

    #[must_use]
    pub fn badge_with_style(label: impl Into<String>, style: ViewStyle) -> Self {
        Self::node(badge_with_style(label, style))
    }

    #[must_use]
    pub fn button(label: impl Into<String>, action_id: impl Into<String>) -> Self {
        Self::node(button(label, action_id))
    }

    #[must_use]
    pub fn button_with_value(
        label: impl Into<String>,
        action_id: impl Into<String>,
        action_value: impl Into<String>,
    ) -> Self {
        Self::node(button_with_value(label, action_id, action_value))
    }

    #[must_use]
    pub fn checkbox(label: impl Into<String>, checked: bool, action_id: impl Into<String>) -> Self {
        Self::node(checkbox(label, checked, action_id))
    }

    #[must_use]
    pub fn toggle(label: impl Into<String>, enabled: bool, action_id: impl Into<String>) -> Self {
        Self::node(toggle(label, enabled, action_id))
    }

    #[must_use]
    pub fn select(
        label: impl Into<String>,
        action_id: impl Into<String>,
        options: impl IntoIterator<Item = SelectOption>,
        selected: Option<impl Into<String>>,
    ) -> Self {
        Self::node(select(label, action_id, options, selected))
    }

    #[must_use]
    pub fn progress(label: impl Into<String>, value: u64, total: Option<u64>) -> Self {
        Self::node(progress(label, value, total))
    }

    #[must_use]
    pub fn link(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self::node(link(label, url))
    }

    #[must_use]
    pub fn image(src: impl Into<String>, alt: impl Into<String>) -> Self {
        Self::node(image(src, alt))
    }

    #[must_use]
    pub fn image_with_style(
        src: impl Into<String>,
        alt: impl Into<String>,
        height: Option<u16>,
        aspect_ratio: Option<(u16, u16)>,
        fit: ImageFit,
        corner_radius: Option<u16>,
    ) -> Self {
        Self::node(image_with_style(
            src,
            alt,
            height,
            aspect_ratio,
            fit,
            corner_radius,
        ))
    }

    #[must_use]
    pub fn image_with_options(
        src: impl Into<String>,
        alt: impl Into<String>,
        options: ImageOptions,
    ) -> Self {
        Self::node(image_with_options(src, alt, options))
    }

    #[must_use]
    pub fn finish(self) -> ViewTree {
        ViewTreeBuilder::new().finish(self)
    }
}

impl From<ViewNode> for View {
    fn from(root: ViewNode) -> Self {
        Self::node(root)
    }
}

impl From<Container> for View {
    fn from(container: Container) -> Self {
        container.finish_view()
    }
}

#[derive(Clone, Debug)]
pub struct Container {
    style: ViewStyle,
    children: Vec<View>,
}

impl Container {
    #[must_use]
    pub fn new(direction: LayoutDirection) -> Self {
        Self {
            style: ViewStyle {
                direction,
                padding: 24,
                gap: 12,
                ..default_style()
            },
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn padding(mut self, padding: u16) -> Self {
        self.style.padding = padding;
        self
    }

    #[must_use]
    pub fn gap(mut self, gap: u16) -> Self {
        self.style.gap = gap;
        self
    }

    #[must_use]
    pub fn align(mut self, align: Align) -> Self {
        self.style.align = align;
        self
    }

    #[must_use]
    pub fn background(mut self, token: ThemeToken) -> Self {
        self.style.background = Some(token);
        self
    }

    #[must_use]
    pub fn full_width(mut self) -> Self {
        self.style.full_width = true;
        self
    }

    #[must_use]
    pub fn corner_radius(mut self, radius: u16) -> Self {
        self.style.corner_radius = Some(radius);
        self
    }

    #[must_use]
    pub fn child(mut self, child: impl Into<View>) -> Self {
        self.children.push(child.into());
        self
    }

    #[must_use]
    pub fn finish_view(self) -> View {
        View {
            node: ViewNode::Container(ContainerNode {
                style: self.style,
                children: Vec::new(),
            }),
            children: self.children,
        }
    }

    #[must_use]
    pub fn finish(self) -> ViewTree {
        self.finish_view().finish()
    }
}

#[must_use]
pub fn card(children: impl IntoIterator<Item = View>) -> View {
    let mut container = View::column()
        .padding(14)
        .gap(10)
        .background(ThemeToken::Surface)
        .corner_radius(8)
        .full_width();
    for child in children {
        container = container.child(child);
    }
    container.finish_view()
}

#[must_use]
pub fn section(title_text: impl Into<String>, children: impl IntoIterator<Item = View>) -> View {
    let mut container = View::column()
        .padding(0)
        .gap(10)
        .full_width()
        .child(View::title(title_text));
    for child in children {
        container = container.child(child);
    }
    container.finish_view()
}

#[derive(Clone, Debug, Default)]
pub struct ViewTreeBuilder {
    nodes: Vec<ViewNode>,
}

impl ViewTreeBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn finish(mut self, root: View) -> ViewTree {
        let root = self.push_recursive(root);
        ViewTree {
            root,
            nodes: self.nodes,
        }
    }

    fn push_recursive(&mut self, view: View) -> u32 {
        let index = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        self.nodes.push(view.node);
        let child_indexes = view
            .children
            .into_iter()
            .map(|child| self.push_recursive(child))
            .collect::<Vec<_>>();

        if let Ok(node_index) = usize::try_from(index) {
            match &mut self.nodes[node_index] {
                ViewNode::Container(container) => {
                    container.children = child_indexes;
                }
                ViewNode::ItemList(list) => {
                    list.items = child_indexes;
                }
                ViewNode::Text(_)
                | ViewNode::Button(_)
                | ViewNode::Input(_)
                | ViewNode::Checkbox(_)
                | ViewNode::Toggle(_)
                | ViewNode::Select(_)
                | ViewNode::Progress(_)
                | ViewNode::Link(_)
                | ViewNode::Separator
                | ViewNode::Badge(_)
                | ViewNode::Icon(_)
                | ViewNode::Image(_)
                | ViewNode::Spacer(_) => {}
            }
        }

        index
    }
}

#[must_use]
pub fn default_style() -> ViewStyle {
    ViewStyle {
        direction: LayoutDirection::Column,
        gap: 8,
        padding: 0,
        align: Align::Start,
        color: None,
        background: None,
        text_size: TextSizeToken::Body,
        emphasis: false,
        full_width: false,
        corner_radius: None,
    }
}

#[must_use]
pub fn title_style() -> ViewStyle {
    ViewStyle {
        text_size: TextSizeToken::Title,
        emphasis: true,
        ..default_style()
    }
}

#[must_use]
pub fn accent_button_style() -> ViewStyle {
    ViewStyle {
        color: Some(ThemeToken::Accent),
        emphasis: true,
        corner_radius: Some(8),
        ..default_style()
    }
}

#[must_use]
pub fn container(children: Vec<u32>) -> ViewNode {
    ViewNode::Container(ContainerNode {
        style: ViewStyle {
            padding: 24,
            gap: 12,
            ..default_style()
        },
        children,
    })
}

#[must_use]
pub fn text(text: impl Into<String>) -> ViewNode {
    ViewNode::Text(TextNode {
        text: text.into(),
        style: default_style(),
    })
}

#[must_use]
pub fn title(text: impl Into<String>) -> ViewNode {
    ViewNode::Text(TextNode {
        text: text.into(),
        style: title_style(),
    })
}

#[must_use]
pub fn badge(label: impl Into<String>) -> ViewNode {
    ViewNode::Badge(BadgeNode {
        label: label.into(),
        style: default_style(),
    })
}

#[must_use]
pub fn badge_with_style(label: impl Into<String>, style: ViewStyle) -> ViewNode {
    ViewNode::Badge(BadgeNode {
        label: label.into(),
        style,
    })
}

#[must_use]
pub fn button(label: impl Into<String>, action_id: impl Into<String>) -> ViewNode {
    ViewNode::Button(ButtonNode {
        label: label.into(),
        action_id: action_id.into(),
        action_value: None,
        style: accent_button_style(),
    })
}

#[must_use]
pub fn button_with_value(
    label: impl Into<String>,
    action_id: impl Into<String>,
    action_value: impl Into<String>,
) -> ViewNode {
    ViewNode::Button(ButtonNode {
        label: label.into(),
        action_id: action_id.into(),
        action_value: Some(action_value.into()),
        style: accent_button_style(),
    })
}

#[must_use]
pub fn checkbox(label: impl Into<String>, checked: bool, action_id: impl Into<String>) -> ViewNode {
    ViewNode::Checkbox(CheckboxNode {
        label: label.into(),
        checked,
        action_id: action_id.into(),
        action_value: Some((!checked).to_string()),
        style: default_style(),
    })
}

#[must_use]
pub fn toggle(label: impl Into<String>, enabled: bool, action_id: impl Into<String>) -> ViewNode {
    ViewNode::Toggle(ToggleNode {
        label: label.into(),
        enabled,
        action_id: action_id.into(),
        action_value: Some((!enabled).to_string()),
        style: default_style(),
    })
}

#[must_use]
pub fn select(
    label: impl Into<String>,
    action_id: impl Into<String>,
    options: impl IntoIterator<Item = SelectOption>,
    selected: Option<impl Into<String>>,
) -> ViewNode {
    ViewNode::Select(SelectNode {
        label: label.into(),
        action_id: action_id.into(),
        options: options.into_iter().collect(),
        selected: selected.map(Into::into),
        style: default_style(),
    })
}

#[must_use]
pub fn option(label: impl Into<String>, value: impl Into<String>) -> SelectOption {
    SelectOption {
        label: label.into(),
        value: value.into(),
    }
}

#[must_use]
pub fn progress(label: impl Into<String>, value: u64, total: Option<u64>) -> ViewNode {
    ViewNode::Progress(ProgressNode {
        label: label.into(),
        value,
        total,
        style: default_style(),
    })
}

#[must_use]
pub fn link(label: impl Into<String>, url: impl Into<String>) -> ViewNode {
    ViewNode::Link(LinkNode {
        label: label.into(),
        url: url.into(),
        tooltip: None,
        style: ViewStyle {
            color: Some(ThemeToken::Accent),
            ..default_style()
        },
    })
}

#[must_use]
pub fn link_with_tooltip(
    label: impl Into<String>,
    url: impl Into<String>,
    tooltip: impl Into<String>,
) -> ViewNode {
    ViewNode::Link(LinkNode {
        label: label.into(),
        url: url.into(),
        tooltip: Some(tooltip.into()),
        style: ViewStyle {
            color: Some(ThemeToken::Accent),
            ..default_style()
        },
    })
}

#[must_use]
pub fn icon(name: impl Into<String>) -> ViewNode {
    ViewNode::Icon(IconNode {
        name: name.into(),
        style: default_style(),
    })
}

#[must_use]
pub fn image(src: impl Into<String>, alt: impl Into<String>) -> ViewNode {
    ViewNode::Image(ImageNode {
        src: src.into(),
        alt: alt.into(),
        caption: String::new(),
        placeholder: String::new(),
        fallback: String::new(),
        style: default_style(),
        height: None,
        min_height: None,
        max_height: None,
        aspect_ratio_x: None,
        aspect_ratio_y: None,
        corner_radius: None,
        fit: ImageFit::Cover,
    })
}

#[must_use]
pub fn image_with_style(
    src: impl Into<String>,
    alt: impl Into<String>,
    height: Option<u16>,
    aspect_ratio: Option<(u16, u16)>,
    fit: ImageFit,
    corner_radius: Option<u16>,
) -> ViewNode {
    let mut options = ImageOptions::new().fit(fit);
    options.height = height;
    options.aspect_ratio_x = aspect_ratio.map(|(x, _)| x);
    options.aspect_ratio_y = aspect_ratio.map(|(_, y)| y);
    options.corner_radius = corner_radius;
    image_with_options(src, alt, options)
}

#[must_use]
pub fn image_with_options(
    src: impl Into<String>,
    alt: impl Into<String>,
    options: ImageOptions,
) -> ViewNode {
    ViewNode::Image(ImageNode {
        src: src.into(),
        alt: alt.into(),
        caption: options.caption,
        placeholder: options.placeholder,
        fallback: options.fallback,
        style: default_style(),
        height: options.height,
        min_height: options.min_height,
        max_height: options.max_height,
        aspect_ratio_x: options.aspect_ratio_x,
        aspect_ratio_y: options.aspect_ratio_y,
        corner_radius: options.corner_radius,
        fit: options.fit,
    })
}

#[must_use]
pub fn spacer(size: u16) -> ViewNode {
    ViewNode::Spacer(SpacerNode { size })
}

fn plugin_error_from_host_error(error: HostError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
    }
}

pub fn log(level: LogLevel, message: impl AsRef<str>) {
    let request = HostRequest::Log {
        level,
        message: message.as_ref().to_string(),
    };
    let _ = host_call_unit(HostOp::Log, &request);
}

pub fn log_debug(message: impl AsRef<str>) {
    log(LogLevel::Debug, message);
}

pub fn log_info(message: impl AsRef<str>) {
    log(LogLevel::Info, message);
}

pub fn log_warn(message: impl AsRef<str>) {
    log(LogLevel::Warn, message);
}

pub fn log_error(message: impl AsRef<str>) {
    log(LogLevel::Error, message);
}

pub fn show_toast(kind: ToastKind, message: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::ShowToast,
        &HostRequest::ShowToast {
            kind,
            message: message.as_ref().to_string(),
        },
    )
}

pub fn open_window(request: &WindowRequest) -> PluginResult<u64> {
    match host_call(
        HostOp::OpenWindow,
        &HostRequest::OpenWindow {
            request: request.clone(),
        },
    )? {
        HostResponse::WindowId(window_id) => Ok(window_id),
        HostResponse::Unit
        | HostResponse::String(_)
        | HostResponse::U64(_)
        | HostResponse::SessionValue(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::ThemeSnapshot(_)
        | HostResponse::HttpTextResponse { .. } => Err(plugin_error(
            "invalid-host-response",
            "open-window returned unexpected response type",
        )),
    }
}

pub fn open_modal(request: &ModalRequest) -> PluginResult<()> {
    host_call_unit(
        HostOp::OpenModal,
        &HostRequest::OpenModal {
            request: request.clone(),
        },
    )
}

pub fn navigate(target: &RouteTarget) -> PluginResult<()> {
    host_call_unit(
        HostOp::Navigate,
        &HostRequest::Navigate {
            target: target.clone(),
        },
    )
}

pub fn navigate_plugin(
    plugin_id: impl Into<String>,
    page_id: impl Into<String>,
) -> PluginResult<()> {
    navigate(&RouteTarget {
        plugin_id: Some(plugin_id.into()),
        page_id: Some(page_id.into()),
        path: String::new(),
    })
}

pub fn navigate_page(plugin: PluginMetadata, page_id: impl Into<String>) -> PluginResult<()> {
    navigate_plugin(plugin.id, page_id)
}

pub fn navigate_path(path: impl Into<String>) -> PluginResult<()> {
    navigate(&RouteTarget {
        plugin_id: None,
        page_id: None,
        path: path.into(),
    })
}

pub fn emit_event(name: impl AsRef<str>, payload: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::EmitEvent,
        &HostRequest::EmitEvent {
            name: name.as_ref().to_string(),
            payload: payload.as_ref().to_string(),
        },
    )
}

pub fn invalidate(target: InvalidateTarget) -> PluginResult<()> {
    host_call_unit(HostOp::Invalidate, &HostRequest::Invalidate { target })
}

pub fn invalidate_all() -> PluginResult<()> {
    invalidate(InvalidateTarget::All)
}

pub fn invalidate_page(page_id: impl Into<String>) -> PluginResult<()> {
    invalidate(InvalidateTarget::Page(page_id.into()))
}

pub fn invalidate_injection(
    slot: InjectionSlot,
    page: Option<impl Into<String>>,
) -> PluginResult<()> {
    invalidate(InvalidateTarget::Injection(InjectionRequest {
        slot,
        page: page.map(Into::into),
    }))
}

#[must_use]
pub fn current_locale() -> String {
    match host_call(HostOp::CurrentLocale, &HostRequest::CurrentLocale) {
        Ok(HostResponse::String(locale)) => locale,
        Ok(
            HostResponse::Unit
            | HostResponse::WindowId(_)
            | HostResponse::U64(_)
            | HostResponse::SessionValue(_)
            | HostResponse::Bytes(_)
            | HostResponse::StringList(_)
            | HostResponse::TaskId(_)
            | HostResponse::AppInfo(_)
            | HostResponse::ThemeSnapshot(_)
            | HostResponse::HttpTextResponse { .. },
        )
        | Err(_) => String::new(),
    }
}

#[must_use]
pub fn tr(key: impl AsRef<str>) -> String {
    tr_args(key, &[])
}

#[must_use]
pub fn tr_args(key: impl AsRef<str>, args: &[I18nArg]) -> String {
    match host_call(
        HostOp::Translate,
        &HostRequest::Translate {
            key: key.as_ref().to_string(),
            args: args.to_vec(),
        },
    ) {
        Ok(HostResponse::String(value)) => value,
        Ok(
            HostResponse::Unit
            | HostResponse::WindowId(_)
            | HostResponse::U64(_)
            | HostResponse::SessionValue(_)
            | HostResponse::Bytes(_)
            | HostResponse::StringList(_)
            | HostResponse::TaskId(_)
            | HostResponse::AppInfo(_)
            | HostResponse::ThemeSnapshot(_)
            | HostResponse::HttpTextResponse { .. },
        )
        | Err(_) => key.as_ref().to_string(),
    }
}

#[must_use]
pub fn tr_arg(key: impl Into<String>, value: impl Into<String>) -> I18nArg {
    I18nArg {
        key: key.into(),
        value: value.into(),
    }
}

pub fn read_config() -> PluginResult<String> {
    match host_call(HostOp::ReadConfig, &HostRequest::ReadConfig)? {
        HostResponse::String(text) => Ok(text),
        HostResponse::Unit
        | HostResponse::WindowId(_)
        | HostResponse::U64(_)
        | HostResponse::SessionValue(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::ThemeSnapshot(_)
        | HostResponse::HttpTextResponse { .. } => Err(plugin_error(
            "invalid-host-response",
            "read-config returned unexpected response type",
        )),
    }
}

pub fn http_get_text(
    url: impl AsRef<str>,
    ttl_seconds: u32,
    max_bytes: u32,
) -> PluginResult<HttpTextResponse> {
    match host_call(
        HostOp::HttpGetText,
        &HostRequest::HttpGetText {
            url: url.as_ref().to_string(),
            ttl_seconds,
            max_bytes,
        },
    )? {
        HostResponse::HttpTextResponse {
            state,
            body,
            error,
            fetched_at_unix_ms,
        } => Ok(HttpTextResponse {
            state,
            body,
            error,
            fetched_at_unix_ms,
        }),
        HostResponse::Unit
        | HostResponse::WindowId(_)
        | HostResponse::String(_)
        | HostResponse::U64(_)
        | HostResponse::SessionValue(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::ThemeSnapshot(_) => Err(plugin_error(
            "invalid-host-response",
            "http-get-text returned unexpected response type",
        )),
    }
}

pub fn write_clipboard_text(text: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::WriteClipboardText,
        &HostRequest::WriteClipboardText {
            text: text.as_ref().to_string(),
        },
    )
}

pub fn read_clipboard_text() -> PluginResult<Option<String>> {
    match host_call(HostOp::ReadClipboardText, &HostRequest::ReadClipboardText)? {
        HostResponse::SessionValue(value) => Ok(value),
        other => unexpected_host_response(other, "read-clipboard-text"),
    }
}

pub fn current_unix_ms() -> PluginResult<u64> {
    match host_call(HostOp::CurrentUnixMs, &HostRequest::CurrentUnixMs)? {
        HostResponse::U64(value) => Ok(value),
        HostResponse::Unit
        | HostResponse::WindowId(_)
        | HostResponse::String(_)
        | HostResponse::SessionValue(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::ThemeSnapshot(_)
        | HostResponse::HttpTextResponse { .. } => Err(plugin_error(
            "invalid-host-response",
            "current-unix-ms returned unexpected response type",
        )),
    }
}

pub fn open_external_url(url: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::OpenExternalUrl,
        &HostRequest::OpenExternalUrl {
            url: url.as_ref().to_string(),
        },
    )
}

pub fn read_resource_text(path: impl AsRef<str>) -> PluginResult<String> {
    match host_call(
        HostOp::ReadResourceText,
        &HostRequest::ReadResourceText {
            path: path.as_ref().to_string(),
        },
    )? {
        HostResponse::String(text) => Ok(text),
        other => unexpected_host_response(other, "read-resource-text"),
    }
}

pub fn read_resource_bytes(path: impl AsRef<str>) -> PluginResult<Vec<u8>> {
    match host_call(
        HostOp::ReadResourceBytes,
        &HostRequest::ReadResourceBytes {
            path: path.as_ref().to_string(),
        },
    )? {
        HostResponse::Bytes(bytes) => Ok(bytes),
        other => unexpected_host_response(other, "read-resource-bytes"),
    }
}

pub fn session_get(key: impl AsRef<str>) -> PluginResult<Option<String>> {
    match host_call(
        HostOp::SessionGet,
        &HostRequest::SessionGet {
            key: key.as_ref().to_string(),
        },
    )? {
        HostResponse::SessionValue(value) => Ok(value),
        HostResponse::Unit
        | HostResponse::WindowId(_)
        | HostResponse::String(_)
        | HostResponse::U64(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::ThemeSnapshot(_)
        | HostResponse::HttpTextResponse { .. } => Err(plugin_error(
            "invalid-host-response",
            "session-get returned unexpected response type",
        )),
    }
}

pub fn session_set(key: impl AsRef<str>, value: Option<impl AsRef<str>>) -> PluginResult<()> {
    host_call_unit(
        HostOp::SessionSet,
        &HostRequest::SessionSet {
            key: key.as_ref().to_string(),
            value: value.map(|value| value.as_ref().to_string()),
        },
    )
}

pub fn storage_get(key: impl AsRef<str>) -> PluginResult<Option<String>> {
    match host_call(
        HostOp::StorageGet,
        &HostRequest::StorageGet {
            key: key.as_ref().to_string(),
        },
    )? {
        HostResponse::SessionValue(value) => Ok(value),
        other => unexpected_host_response(other, "storage-get"),
    }
}

pub fn storage_set(key: impl AsRef<str>, value: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::StorageSet,
        &HostRequest::StorageSet {
            key: key.as_ref().to_string(),
            value: value.as_ref().to_string(),
        },
    )
}

pub fn storage_delete(key: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::StorageDelete,
        &HostRequest::StorageDelete {
            key: key.as_ref().to_string(),
        },
    )
}

pub fn storage_list(prefix: Option<impl AsRef<str>>) -> PluginResult<Vec<String>> {
    match host_call(
        HostOp::StorageList,
        &HostRequest::StorageList {
            prefix: prefix.map(|prefix| prefix.as_ref().to_string()),
        },
    )? {
        HostResponse::StringList(keys) => Ok(keys),
        other => unexpected_host_response(other, "storage-list"),
    }
}

pub fn config_read() -> PluginResult<String> {
    read_config()
}

pub fn config_write(text: impl AsRef<str>) -> PluginResult<()> {
    host_call_unit(
        HostOp::WriteConfig,
        &HostRequest::WriteConfig {
            text: text.as_ref().to_string(),
        },
    )
}

pub fn create_task(request: TaskCreateRequest) -> PluginResult<String> {
    match host_call(HostOp::CreateTask, &HostRequest::CreateTask { request })? {
        HostResponse::TaskId(task_id) => Ok(task_id),
        other => unexpected_host_response(other, "create-task"),
    }
}

pub fn update_task(request: TaskUpdateRequest) -> PluginResult<()> {
    host_call_unit(HostOp::UpdateTask, &HostRequest::UpdateTask { request })
}

pub fn finish_task(request: TaskFinishRequest) -> PluginResult<()> {
    host_call_unit(HostOp::FinishTask, &HostRequest::FinishTask { request })
}

pub fn app_info() -> PluginResult<AppInfo> {
    match host_call(HostOp::AppInfo, &HostRequest::AppInfo)? {
        HostResponse::AppInfo(info) => Ok(info),
        other => unexpected_host_response(other, "app-info"),
    }
}

pub fn theme_snapshot() -> PluginResult<ThemeSnapshot> {
    match host_call(HostOp::ThemeSnapshot, &HostRequest::ThemeSnapshot)? {
        HostResponse::ThemeSnapshot(snapshot) => Ok(snapshot),
        HostResponse::Unit
        | HostResponse::WindowId(_)
        | HostResponse::String(_)
        | HostResponse::U64(_)
        | HostResponse::SessionValue(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::HttpTextResponse { .. } => Err(plugin_error(
            "invalid-host-response",
            "theme-snapshot returned unexpected response type",
        )),
    }
}

#[must_use]
pub fn is_dark_mode() -> bool {
    theme_snapshot()
        .map(ThemeSnapshot::is_dark)
        .unwrap_or(false)
}

#[must_use]
pub fn theme_accent() -> Option<ThemeColor> {
    theme_snapshot().ok().map(|snapshot| snapshot.accent)
}

pub fn plugin_error(code: impl Into<String>, message: impl Into<String>) -> PluginError {
    PluginError::new(code, message)
}

fn unexpected_host_response<T>(
    _response: HostResponse,
    operation: &'static str,
) -> PluginResult<T> {
    Err(plugin_error(
        "invalid-host-response",
        format!("{operation} returned unexpected response type"),
    ))
}

fn host_call_unit(op: HostOp, request: &HostRequest) -> PluginResult<()> {
    match host_call(op, request)? {
        HostResponse::Unit => Ok(()),
        HostResponse::WindowId(_)
        | HostResponse::String(_)
        | HostResponse::U64(_)
        | HostResponse::SessionValue(_)
        | HostResponse::Bytes(_)
        | HostResponse::StringList(_)
        | HostResponse::TaskId(_)
        | HostResponse::AppInfo(_)
        | HostResponse::ThemeSnapshot(_)
        | HostResponse::HttpTextResponse { .. } => Err(plugin_error(
            "invalid-host-response",
            "host returned unexpected response type",
        )),
    }
}

fn host_call(op: HostOp, request: &HostRequest) -> PluginResult<HostResponse> {
    let request_bytes = postcard::to_allocvec(request).map_err(|error| {
        plugin_error(
            "postcard-encode-failed",
            format!("encode host request failed: {error}"),
        )
    })?;
    let mut response = vec![0_u8; DEFAULT_HOST_BUFFER_CAPACITY];

    loop {
        let written = unsafe_host_call(
            op.code(),
            request_bytes.as_ptr(),
            request_bytes.len(),
            response.as_mut_ptr(),
            response.len(),
        );

        if written < 0 {
            let required = usize::try_from(-written).unwrap_or(MAX_HOST_BUFFER_CAPACITY + 1);
            if required == 0 || required > MAX_HOST_BUFFER_CAPACITY {
                return Err(plugin_error(
                    "host-buffer-too-large",
                    format!("host requested invalid response size {required}"),
                ));
            }
            response.resize(required, 0);
            continue;
        }

        let written = usize::try_from(written).map_err(|_error| {
            plugin_error(
                "host-response-invalid",
                "host returned a negative or overflowing response size",
            )
        })?;
        if written > response.len() {
            return Err(plugin_error(
                "host-response-out-of-bounds",
                "host wrote more bytes than the provided response buffer",
            ));
        }

        let response_value: Result<HostResponse, HostError> =
            postcard::from_bytes(&response[..written]).map_err(|error| {
                plugin_error(
                    "postcard-decode-failed",
                    format!("decode host response failed: {error}"),
                )
            })?;
        return response_value.map_err(plugin_error_from_host_error);
    }
}

#[cfg(target_arch = "wasm32")]
fn unsafe_host_call(
    op: i32,
    request_ptr: *const u8,
    request_len: usize,
    response_ptr: *mut u8,
    response_capacity: usize,
) -> i64 {
    // SAFETY: The imported function is provided by the BMCBL host and follows the ABI declared
    // below. All pointers refer to this plugin instance's linear memory for the duration of the
    // synchronous call.
    unsafe {
        bmcbl_host_call(
            op,
            i32::try_from(request_ptr as usize).unwrap_or(i32::MAX),
            i32::try_from(request_len).unwrap_or(i32::MAX),
            i32::try_from(response_ptr as usize).unwrap_or(i32::MAX),
            i32::try_from(response_capacity).unwrap_or(i32::MAX),
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn unsafe_host_call(
    _op: i32,
    _request_ptr: *const u8,
    _request_len: usize,
    _response_ptr: *mut u8,
    _response_capacity: usize,
) -> i64 {
    0
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "bmcbl")]
unsafe extern "C" {
    fn bmcbl_host_call(
        op: i32,
        request_ptr: i32,
        request_len: i32,
        response_ptr: i32,
        response_capacity: i32,
    ) -> i64;
}

#[doc(hidden)]
pub fn encode_plugin_result<T: serde::Serialize>(result: PluginResult<T>) -> u64 {
    let response = match result {
        Ok(value) => AbiResult::Ok(value),
        Err(error) => AbiResult::Err(error),
    };
    let bytes = postcard::to_allocvec(&response).unwrap_or_else(|error| {
        let fallback = AbiResult::<()>::Err(plugin_error(
            "postcard-encode-failed",
            format!("encode plugin response failed: {error}"),
        ));
        postcard::to_allocvec(&fallback).unwrap_or_default()
    });
    export_allocated_bytes(bytes)
}

#[doc(hidden)]
pub fn decode_request<T: serde::de::DeserializeOwned>(ptr: u32, len: u32) -> PluginResult<T> {
    // SAFETY: The host always passes a guest-memory pointer/length pair for the current module
    // call. The resulting slice is only used synchronously for postcard decoding.
    let slice = unsafe { alloc::slice::from_raw_parts(ptr as *const u8, len as usize) };
    postcard::from_bytes(slice).map_err(|error| {
        plugin_error(
            "postcard-decode-failed",
            format!("decode plugin request failed: {error}"),
        )
    })
}

fn export_allocated_bytes(bytes: Vec<u8>) -> u64 {
    let mut bytes = bytes.into_boxed_slice();
    let len = bytes.len();
    let ptr = bytes.as_mut_ptr();
    core::mem::forget(bytes);
    (u64::from(ptr as u32) << 32) | u64::from(len as u32)
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn bmcbl_alloc(len: u32, _align: u32) -> u32 {
    if len == 0 {
        return 0;
    }
    let mut bytes = Vec::<u8>::with_capacity(len as usize);
    let ptr = bytes.as_mut_ptr();
    core::mem::forget(bytes);
    ptr as u32
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn bmcbl_dealloc(ptr: u32, len: u32, _align: u32) {
    if ptr == 0 || len == 0 {
        return;
    }
    // SAFETY: `ptr` must have been returned by `bmcbl_alloc` or by the plugin response allocator.
    // The ABI always passes back the original allocation length, which is the allocation capacity
    // for this byte buffer.
    unsafe {
        let _ = Vec::from_raw_parts(ptr as *mut u8, 0, len as usize);
    }
}

#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn bmcbl_init(ptr: u32, len: u32) -> u64 {
            let context = match $crate::decode_request::<$crate::PluginContext>(ptr, len) {
                Ok(context) => context,
                Err(error) => {
                    return $crate::encode_plugin_result::<Vec<$crate::Registration>>(Err(error))
                }
            };
            $crate::encode_plugin_result(<$plugin as $crate::Plugin>::init(context))
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn bmcbl_handle_event(ptr: u32, len: u32) -> u64 {
            let event = match $crate::decode_request::<$crate::HostEvent>(ptr, len) {
                Ok(event) => event,
                Err(error) => return $crate::encode_plugin_result::<()>(Err(error)),
            };
            $crate::encode_plugin_result(<$plugin as $crate::Plugin>::handle_event(event))
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn bmcbl_render_page(ptr: u32, len: u32) -> u64 {
            let request = match $crate::decode_request::<$crate::PageRenderRequest>(ptr, len) {
                Ok(request) => request,
                Err(error) => return $crate::encode_plugin_result::<$crate::ViewTree>(Err(error)),
            };
            $crate::encode_plugin_result(<$plugin as $crate::Plugin>::render_page(request))
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn bmcbl_render_injection(ptr: u32, len: u32) -> u64 {
            let request = match $crate::decode_request::<$crate::InjectionRequest>(ptr, len) {
                Ok(request) => request,
                Err(error) => {
                    return $crate::encode_plugin_result::<Option<$crate::ViewTree>>(Err(error));
                }
            };
            $crate::encode_plugin_result(<$plugin as $crate::Plugin>::render_injection(request))
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn bmcbl_shutdown(ptr: u32, len: u32) -> u64 {
            let reason = match $crate::decode_request::<$crate::ShutdownReason>(ptr, len) {
                Ok(reason) => reason,
                Err(error) => return $crate::encode_plugin_result::<()>(Err(error)),
            };
            $crate::encode_plugin_result(<$plugin as $crate::Plugin>::shutdown(reason))
        }
    };
}

#[macro_export]
macro_rules! plugin_actions {
    (
        $(#[$meta:meta])*
        $visibility:vis enum $name:ident {
            $($variant:ident = $action_id:literal),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $visibility enum $name {
            $($variant),*
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $action_id),*
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

#[macro_export]
macro_rules! registrations {
    () => {
        $crate::Registrations::new().finish()
    };
    (page $page_id:expr, title = $title:expr, nav = $nav:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .page($crate::Page::new($page_id, $title).nav($nav))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (page $page_id:expr, title = $title:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .page($crate::Page::new($page_id, $title))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, priority = $priority:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).priority($priority))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, page = $page:expr, priority = $priority:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).page($page).priority($priority))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, page = $page:expr, priority = $priority:expr, layout = $layout:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).page($page).priority($priority).layout($layout))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, priority = $priority:expr, layout = $layout:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).priority($priority).layout($layout))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (subscribe $event:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .subscribe($event)
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    ($($entry:expr),* $(,)?) => {
        $crate::Registrations::new()
            $(.register($entry))*
            .finish()
    };
}

#[macro_export]
macro_rules! view {
    (column($($name:ident = $value:expr),* $(,)?) { $($child:expr);* $(;)? }) => {{
        let view = $crate::View::column()
            $(.$name($value))*
            $(.child($child))*
            .finish();
        view
    }};
    (row($($name:ident = $value:expr),* $(,)?) { $($child:expr);* $(;)? }) => {{
        let view = $crate::View::row()
            $(.$name($value))*
            $(.child($child))*
            .finish();
        view
    }};
    ($node:expr) => {
        $crate::View::from($node).finish()
    };
}

#[macro_export]
macro_rules! toast {
    (info, $message:expr) => {
        $crate::show_toast($crate::ToastKind::Info, $message)
    };
    (success, $message:expr) => {
        $crate::show_toast($crate::ToastKind::Success, $message)
    };
    (error, $message:expr) => {
        $crate::show_toast($crate::ToastKind::Error, $message)
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::log_debug(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log_info(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log_warn(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::log_error(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::tr($key)
    };
    ($key:expr, $( $arg_key:expr => $arg_value:expr ),+ $(,)?) => {{
        let args = vec![$($crate::tr_arg($arg_key, format!("{}", $arg_value))),+];
        $crate::tr_args($key, &args)
    }};
}

#[macro_export]
macro_rules! invalidate {
    (all) => {
        $crate::invalidate_all()
    };
    (page $page_id:expr) => {
        $crate::invalidate_page($page_id)
    };
    (injection $slot:expr) => {
        $crate::invalidate_injection($slot, Option::<String>::None)
    };
    (injection $slot:expr, page = $page:expr) => {
        $crate::invalidate_injection($slot, Some($page))
    };
}

pub mod prelude {
    pub use crate::{
        AbiResult, Align, CompactBehavior, Container, EventSubscription, HostEvent, HostEventKind,
        HttpCacheState, HttpTextResponse, I18nArg, ImageFit, ImageOptions, Injection,
        InjectionLayout, InjectionRegistration, InjectionRequest, InjectionSlot, LogLevel, Modal,
        ModalRequest, Nav, Page, PageRegistration, PageRenderRequest, Plugin, PluginContext,
        PluginError, PluginMetadata, PluginResult, Registration, Registrations, SelectOption,
        ShutdownReason, StorageEntry, TaskCreateRequest, TaskFinishRequest, TaskUpdateRequest,
        TextSizeToken, ThemeColor, ThemeMode, ThemeSnapshot, ThemeToken, ToastKind, View, ViewNode,
        ViewStyle, ViewTree, ViewTreeBuilder, Window, app_info, badge, badge_with_style,
        bmcbl_plugin, button, button_with_value, card, checkbox, config_read, config_write,
        create_task, current_locale, current_unix_ms, default_style, emit_event, finish_task,
        http_get_text, icon, image, image_with_options, image_with_style, invalidate,
        invalidate_all, invalidate_injection, invalidate_page, is_dark_mode, link,
        link_with_tooltip, log, log_debug, log_error, log_info, log_warn, navigate, navigate_page,
        navigate_path, open_external_url, open_modal, open_window, option, plugin_actions,
        plugin_error, plugin_metadata, progress, read_clipboard_text, read_config,
        read_resource_bytes, read_resource_text, registrations, section, select, session_get,
        session_set, show_toast, spacer, storage_delete, storage_get, storage_list, storage_set,
        text, theme_accent, theme_snapshot, title, toast, toggle, tr, tr_arg, tr_args, update_task,
        view, write_clipboard_text,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_builder_outputs_page_injection_and_subscription() {
        let registrations = Registrations::new()
            .page(Page::new("main", "Main").nav(Nav::new("Essentials").icon("plug").order(7)))
            .injection(Injection::new(InjectionSlot::MainRootOverlay).priority(3))
            .subscribe("route-changed")
            .finish();

        assert_eq!(registrations.len(), 3);
    }

    #[test]
    fn view_macro_builds_root_container_with_children() {
        let tree = view! {
            column(padding = 16, gap = 8) {
                badge("Loaded");
                title("BMCBL Essentials");
                button("Open", "open-window");
            }
        };

        assert_eq!(tree.root, 0);
        assert_eq!(tree.nodes.len(), 4);
        let ViewNode::Container(container) = &tree.nodes[0] else {
            panic!("root should be a container");
        };
        assert_eq!(container.children, vec![1, 2, 3]);
    }

    #[test]
    fn ui_control_helpers_build_expected_nodes() {
        let ViewNode::Checkbox(checkbox) = checkbox("Enabled", true, "toggle-enabled") else {
            panic!("checkbox helper should build checkbox node");
        };
        assert_eq!(checkbox.action_value.as_deref(), Some("false"));

        let ViewNode::Select(select) = select(
            "Mode",
            "mode",
            [option("Fast", "fast"), option("Safe", "safe")],
            Some("safe"),
        ) else {
            panic!("select helper should build select node");
        };
        assert_eq!(select.options.len(), 2);
        assert_eq!(select.selected.as_deref(), Some("safe"));

        let ViewNode::Progress(progress) = progress("Sync", 3, Some(10)) else {
            panic!("progress helper should build progress node");
        };
        assert_eq!(progress.value, 3);
        assert_eq!(progress.total, Some(10));
    }

    #[test]
    fn plugin_error_helpers_set_stable_codes() {
        assert_eq!(PluginError::denied("no").code, "denied");
        assert_eq!(PluginError::invalid_input("bad").code, "invalid-input");
        assert_eq!(PluginError::not_found("missing").code, "not-found");
        assert_eq!(PluginError::host("host").code, "host");
        assert_eq!(PluginError::timeout("slow").code, "timeout");
    }

    #[test]
    fn host_request_response_roundtrip_covers_v04_types() {
        let request = HostRequest::CreateTask {
            request: TaskCreateRequest {
                task_id: None,
                title: "Build".to_string(),
                detail: Some("Plugin task".to_string()),
                stage: "starting".to_string(),
                total: Some(100),
                supports_pause: false,
            },
        };
        let encoded = postcard::to_allocvec(&request).expect("request should encode");
        let decoded = postcard::from_bytes::<HostRequest>(&encoded).expect("request should decode");
        assert_eq!(decoded, request);

        let response = HostResponse::AppInfo(AppInfo {
            version: "1.0.0".to_string(),
            build_info: "build".to_string(),
            api_version: API_VERSION.to_string(),
        });
        let encoded = postcard::to_allocvec(&response).expect("response should encode");
        let decoded =
            postcard::from_bytes::<HostResponse>(&encoded).expect("response should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn action_helper_matches_action_events() {
        let event = HostEvent {
            plugin_id: Some("plugin".to_string()),
            page_id: Some("main".to_string()),
            kind: HostEventKind::Action(ActionEvent {
                action_id: "open-window".to_string(),
                value: None,
            }),
        };

        assert!(event.action_is("open-window"));
    }

    #[test]
    fn image_options_builder_sets_stable_render_fields() {
        let node = image_with_options(
            "https://example.com/image.webp",
            "Preview",
            ImageOptions::new()
                .caption("Caption")
                .placeholder("Loading")
                .fallback("Unavailable")
                .height(180)
                .min_height(120)
                .max_height(240)
                .aspect_ratio(16, 9)
                .corner_radius(12)
                .fit(ImageFit::Contain),
        );

        let ViewNode::Image(image) = node else {
            panic!("node should be an image");
        };
        assert_eq!(image.caption, "Caption");
        assert_eq!(image.placeholder, "Loading");
        assert_eq!(image.fallback, "Unavailable");
        assert_eq!(image.height, Some(180));
        assert_eq!(image.min_height, Some(120));
        assert_eq!(image.max_height, Some(240));
        assert_eq!(image.aspect_ratio_x, Some(16));
        assert_eq!(image.aspect_ratio_y, Some(9));
        assert_eq!(image.corner_radius, Some(12));
        assert_eq!(image.fit, ImageFit::Contain);
    }

    #[test]
    fn injection_layout_builder_roundtrips() {
        let registration = Injection::new(InjectionSlot::HomeSidebar)
            .page("/")
            .priority(40)
            .layout(
                InjectionLayout::sidebar()
                    .width(320)
                    .min_width(248)
                    .max_width(360)
                    .max_height(360)
                    .priority(2)
                    .compact_behavior(CompactBehavior::Scroll),
            )
            .finish();

        let bytes = postcard::to_allocvec(&registration).expect("layout should encode");
        let decoded =
            postcard::from_bytes::<InjectionRegistration>(&bytes).expect("layout should decode");

        assert_eq!(decoded, registration);
        assert_eq!(
            decoded.layout.and_then(|layout| layout.preferred_width),
            Some(320)
        );
    }

    #[test]
    fn theme_snapshot_response_roundtrips() {
        let snapshot = ThemeSnapshot {
            dark_factor: 1.0,
            mode: ThemeMode::Dark,
            accent: ThemeColor {
                h: 0.6,
                s: 0.8,
                l: 0.5,
                a: 1.0,
            },
            ..ThemeSnapshot::light_default()
        };
        let response = HostResponse::ThemeSnapshot(snapshot);

        let bytes = postcard::to_allocvec(&response).expect("theme snapshot should encode");
        let decoded =
            postcard::from_bytes::<HostResponse>(&bytes).expect("theme snapshot should decode");

        assert_eq!(decoded, response);
        assert!(snapshot.is_dark());
    }
}
