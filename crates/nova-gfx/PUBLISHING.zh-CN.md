# nova-gfx 发布说明

本文档记录 `nova-gfx` crate 家族的 crates.io 发布形态。

## 发布顺序

按依赖顺序发布：

1. `gfx-core`
2. `gfx-memory`
3. `gfx-shader`
4. `gfx-vulkan`
5. `gfx-dx12`
6. `gfx-metal`

`gfx-memory`、`gfx-shader` 和后端 crate 使用 `version + path` 依赖，方便
本地开发。对这些下游 crate 执行 `cargo package` 时，Cargo 会要求对应依赖
版本已经存在于 registry，因此下游 crate 的 package dry-run 应在依赖发布后
执行。

## metadata 标准

每个库 crate 必须保持：

- `license`
- `description`
- `rust-version`
- `readme`
- `documentation`
- `keywords`
- `categories`
- `include`
- `README.md`
- `README.zh-CN.md`

examples 是 workspace 内的 smoke test，必须保持 `publish = false`。

没有真实公开仓库地址前，不要填写 `repository` 或 `homepage`。发布 metadata
中应避免占位 URL。

## 校验

发布前执行：

```powershell
rtk cargo fmt --all
rtk cargo test -p gfx-core
rtk cargo test -p gfx-memory --no-default-features
rtk cargo test -p gfx-memory --no-default-features --features vulkan
rtk cargo test -p gfx-memory --no-default-features --features dx12
rtk cargo test -p gfx-shader
rtk cargo check -p gfx-vulkan
rtk cargo check -p gfx-dx12
rtk cargo check -p gfx-metal
rtk cargo check -p gpui --no-default-features
rtk cargo check -p gpui --no-default-features --features nova-gfx-vulkan
rtk cargo check -p gpui --no-default-features --features nova-gfx-dx12
rtk cargo tree -p gfx-vulkan --no-default-features
rtk cargo tree -p gpui --no-default-features --features nova-gfx-vulkan
rtk cargo doc -p gfx-core --no-deps
rtk cargo package -p gfx-core --allow-dirty
```

如果发布内容影响 Metal allocator 或 Metal 后端，需要在 macOS 上额外运行
`gfx-memory --features metal`、`gfx-metal` 和 `nova-gfx-metal` 检查。Windows
只能验证 `gfx-metal` 的非 Apple stub 编译路径，不能验证 Metal allocator
feature。

`gfx-vulkan` 和 GPUI Vulkan 的 dependency tree 不应拉入 `gfx-dx12`、
`gfx-metal`、D3D12 或 Metal Objective-C 依赖。

每发布一个依赖 crate 后，再按发布顺序对下一个 crate 执行
`cargo package --allow-dirty`。

## 破坏性 feature 变更

`gfx-memory` 使用 `default = []`，后端 crate 只启用自己需要的 allocator
feature。GPUI 现在必须通过 `nova-gfx-vulkan`、`nova-gfx-dx12` 或
`nova-gfx-metal` 显式选择渲染后端。旧 `nova-gfx` feature 不再选择具体后端，
仅作为下游迁移期的空兼容标记保留。
