# 模糊

[English](backdrop_blur.md)

两种独立的 `Styled` 属性共享 GPU 高斯核，但不互为别名：

| GPUI | Web CSS | 处理对象 |
| --- | --- | --- |
| `.blur(px(3.))` | `filter: blur(3px)` | 元素自身背景、阴影、边框和子元素（含文字） |
| `.backdrop_blur(px(3.))` | `backdrop-filter: blur(3px)` | 此前绘制、位于元素后方的内容 |
| `.bg(rgba(0xffffffcc))` | `background: rgb(255 255 255 / 80%)` | 元素自身填充色 |
| `.opacity(0.5)` | `opacity: .5` | 元素透明度，包含滤镜输出 |

这是参考 CSS 语义的 Rust API，不是 CSS 解析器，也不表示支持所有 CSS 滤镜。
可在 `.id(...)` 后使用；链式调用顺序不改变绘制阶段。
这两个属性不表示已经实现 CSS 滤镜列表解析、任意滤镜组合或浏览器完整的
Backdrop Root/stacking-context 模型。

```rust
use gpui::{div, px, rgba, prelude::*};

let content = div().blur(px(3.)).child("文字也模糊");
let panel = div()
    .id("account-panel")
    .backdrop_blur(px(6.))
    .bg(rgba(0xffffffeb))
    .rounded(px(16.))
    .opacity(0.8)
    .child("文字保持清晰");
```

背景模糊不会自动添加主题色。有色半透明材质使用**同一个元素**的 `.bg(...)`，
不需要额外覆盖层。不透明背景会遮住后方模糊。`BackdropBlurStyle` 还提供质量提示、
饱和度、可选 tint 和重叠策略。tint alpha 与元素 opacity 独立；只淡出 tint
不等于淡出整个滤镜。

长度采用逻辑像素中的高斯标准差 sigma，对应
[CSS Filter Effects](https://www.w3.org/TR/filter-effects-1/#funcdef-filter-blur)。
元素和窗口缩放将它转换为设备像素。有限 GPU 核在三个标准差内近似高斯分布，保留
小数值，但不承诺与浏览器逐像素一致。旧 API 参数为核采样范围；保持原有视觉强度时
将旧值 `r` 迁移成 `r / 3`。

## Scene Data

背景模糊先于自身阴影和填充采样，再绘制背景、子内容和边框，避免采到自身颜色。
圆角与祖先裁剪约束最终结果。元素模糊隔离子树后，在原绘制位置合成过滤结果；效果
范围必须包含溢出子内容与高斯扩散范围。滤镜不改变布局或命中测试。

## GPU Pipeline

Nova 公共 shader 为 [`blur.wgsl`](../src/platform/nova/shaders/blur.wgsl)。
采用水平、垂直两遍处理，CPU 预先计算核权重，避免逐 fragment 求指数。
小核通过硬件双线性过滤合并相邻 tap，每轴最多九次采样；较宽的核用 17 次采样，
避免把不相邻 texel 错误合并。
RGBA 以预乘形式一起过滤，再为着色与最终合成转换颜色，避免透明边缘发黑。

中间 GPU 纹理直接作为 render attachment 和 sampled texture，不做 CPU 回读或
截图上传。子树过滤需要离屏 pass，但应避免多余源复制。背景源按绘制顺序推进，
兼容区域可共用过滤结果；scissor 包含核采样范围。
控制半径、面积和重叠滤镜数，优先对整组内容过滤一次，不要逐行过滤。
面板通常保持半径固定，仅动画 opacity/transform，退场归零后卸载。
硬件加速不等于零带宽成本，不能在未测量时承诺帧率。

## 框架对照与验证

[Flutter BackdropFilter](https://api.flutter.dev/flutter/widgets/BackdropFilter-class.html)
和 [ImageFiltered](https://api.flutter.dev/flutter/widgets/ImageFiltered-class.html)
同样分别处理背景和指定子树。
[Qt Quick MultiEffect](https://doc.qt.io/qt-6/qml-qtquick-effects-multieffect.html)
使用明确 source，并建议限制效果面积及模糊上限。

回归需覆盖父子 opacity、缩放、背景前采样、透明边缘颜色、子树隔离、嵌套、缓存
重放与卸载后一帧。运行 scene/window/Nova 测试，并验证启用后端的 WGSL。
描述符测试不能代替实际像素渲染或交互视觉检查。

## Guidelines

- blur 行为保持 deterministic 且由 renderer 拥有。
- 不要把 application theme defaults 加入 renderer。
- 诊断 blur-heavy windows 时使用 metrics。
- 只有 intentionally retained diagnostic 或 platform compatibility code 才使用局部
  `#[expect(...)]`。
