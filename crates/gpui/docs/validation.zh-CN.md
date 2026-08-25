# 验证

[English](validation.md)

修改 GPUI framework code、examples、docs 或 GPUI skill 时，运行聚焦验证。

## Rust Checks

```powershell
rtk cargo fmt --manifest-path Cargo.toml --all
rtk cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
rtk cargo clippy --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --lib -- -D warnings
rtk cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --examples
rtk cargo bench --manifest-path Cargo.toml --features bench --no-run
```

修复 GPUI scope 内的 warnings。只有 intentional compatibility 或 diagnostic code 才使
用局部 `#[expect(..., reason = "...")]`。

## 支持证据

平台支持按 release 记录实际证据，不能从 `cfg` 声明推断。

| 平台 | 目标 backend | 必需证据 |
| --- | --- | --- |
| Windows（primary） | Nova DX12；显式构建时为 Nova Vulkan | feature check、library tests、benchmark/example 编译和真实窗口 smoke test |
| Linux / FreeBSD | Nova Vulkan | 在声明的 display protocol 上完成 native feature check 和 tests |
| macOS | Nova Metal | native feature check、tests 和真实窗口 smoke test |

`compile-check` workflow 使用 `macos-15` arm64 runner：测试图形契约与 Metal
backend，运行 Metal atlas resource smoke，只启用 `nova-gfx-metal` 检查 GPUI，并呈现
一个自动关闭的 GPUI Metal 窗口。从 Windows 交叉编译只是额外编译门禁，不能替代该
macOS 运行。裸机资格验证仍需要 self-hosted 实体 Mac，并单独记录结果。

每份 release record 必须包含 target triple、OS version、features、Rust version、
GPU/driver、运行时实际 renderer、commands 和 exit codes。如果某一行在该 release 没有
执行，就标记 unverified；不能根据另一平台结果宣称通过。

## 文档检查

检查 GPUI 官方文档是否使用官方库措辞，并采用分离语言文件：

完成前搜索 repository，确认没有本地 vendored-path 叙述，也没有同文件双语 section
headings。

每个 canonical English document 都应在同目录有匹配的 `.zh-CN.md` 文件。GPUI skill
是 English-only，不应包含中文文本。

## 示例检查

示例必须使用当前 GPUI APIs 编译，并避免引用缺失 dependencies。平台专用示例应通过
guarded fallback entry point 在不支持的平台上通过编译。

更新 GPU examples 时，确认 flow 根据渲染发生的位置使用 `removed surface API`、
`back_buffer_view`、`present` 或 `swap_buffers`，以及 `removed surface paint API`。
