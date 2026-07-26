//! 设计令牌：统一圆角 / 间距 / 字号体系。
//!
//! 之前各处散落着 `px(12.)`、`px(14.)`、`px(18.)`、`px(20.)`、`px(24.)`
//! 等硬编码圆角，视觉不一致。新版 UI 统一从这里取值。

/// 圆角体系（px）。
pub mod radius {
    /// 小元素：徽章、内嵌图标底板。
    pub const XS: f32 = 8.0;
    /// 常规控件：按钮、输入框、列表项。
    pub const SM: f32 = 12.0;
    /// 卡片、面板。
    pub const MD: f32 = 16.0;
    /// 浮层、下拉列表。
    pub const LG: f32 = 20.0;
    /// 重点容器：启动栏、模态。
    pub const XL: f32 = 24.0;
    /// 胶囊。
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
