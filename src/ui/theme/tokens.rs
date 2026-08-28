//! 设计令牌：统一圆角 / 间距 / 字号 / 动效体系。
//!
//! 全部 UI 必须从这里取值，禁止在页面/组件里内联硬编码圆角与动画时长。
//! 设计语言：暖纸张中性 + 琥珀橙强调；圆角收敛在 5-8px；动效统一为
//! Apple 风格弹簧（见 `crate::ui::animation`），无悬浮动画。

/// 圆角体系（px）。整体收敛在 5-8px，避免大圆角带来的"AI 感"。
pub mod radius {
    /// 小元素：徽章、标签、内嵌图标底板、进度条。
    pub const XS: f32 = 5.0;
    /// 常规控件：按钮、输入框、下拉、列表项。
    pub const SM: f32 = 6.0;
    /// 卡片、面板、浮层、模态等容器。
    pub const MD: f32 = 8.0;
    /// 与 MD 相同；保留别名以表达"最大容器圆角"的语义。
    pub const LG: f32 = 8.0;
    /// 与 MD 相同；保留别名兼容既有调用点。
    pub const XL: f32 = 8.0;
    /// 胶囊（仅用于开关滑块、圆形图标按钮等本身即圆形的元素）。
    pub const FULL: f32 = 999.0;
}

/// 间距体系（px）。
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 20.0;
    pub const XXL: f32 = 24.0;
}

/// 字号体系（px），参考 Apple HIG 的层级命名。
pub mod font {
    pub const CAPTION: f32 = 11.0;
    pub const FOOTNOTE: f32 = 12.0;
    pub const SUBHEAD: f32 = 13.0;
    pub const BODY: f32 = 15.0;
    pub const HEADLINE: f32 = 17.0;
    pub const TITLE: f32 = 20.0;
}

/// 动效体系。
///
/// 统一规则（避免页面之间/元素之间节奏割裂）：
/// - 页面与面板入场：`ENTRANCE_OFFSET` 向上位移 + 淡入，弹簧用
///   `crate::ui::animation::spring_smooth()`；同一容器内的兄弟元素共享
///   同一条时间线，仅允许 `STAGGER_STEP` 的级联延迟。
/// - 展开类交互（下拉、抽屉、导航胶囊）：`spring_bouncy()`；收起方向
///   一律 `spring_snappy()`，不回弹。
/// - 高频紧凑弹层例外：以触发点为原点轻微缩放，使用 `POPOVER_RESPONSE`
///   的临界阻尼弹簧，不级联、不回弹，避免页面入场节奏拖慢操作。
/// - 按压反馈：`.active(scale(PRESS_SCALE))`，即时生效不加动画。
/// - hover 只做即时颜色切换，禁止位移/缩放/阴影动画。
pub mod motion {
    use std::time::Duration;

    /// 入场位移距离（px）。
    pub const ENTRANCE_OFFSET: f32 = 8.0;
    /// 兄弟元素级联延迟。
    pub const STAGGER_STEP: Duration = Duration::from_millis(30);
    /// 覆盖 `spring_smooth` 稳定所需的一次性动画窗口。
    pub const SMOOTH_WINDOW: Duration = Duration::from_millis(640);
    /// 覆盖 `spring_bouncy` 稳定所需的一次性动画窗口。
    pub const BOUNCY_WINDOW: Duration = Duration::from_millis(840);
    /// 按压缩放比例。
    pub const PRESS_SCALE: f32 = 0.97;
    /// 紧凑弹层的临界阻尼弹簧周期（秒），不是收稳时长。
    /// 配合 position 0.01 / velocity 0.5 的阈值，静止展开约 170ms 收稳。
    pub const POPOVER_RESPONSE: f32 = 0.16;
    /// 选中和短列表反馈的弹簧周期；相同阈值下约 130ms 收稳。
    pub const FEEDBACK_RESPONSE: f32 = 0.12;
    /// 触发点锚定的小弹层只缩放 3%。
    pub const POPOVER_SCALE: f32 = 0.97;
}
