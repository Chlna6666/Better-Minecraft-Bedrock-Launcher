# 验证

[English](validation.md)

修改 GPUI framework code、examples、docs 或 GPUI skill 时，运行聚焦验证。

## Rust Checks

```powershell
cargo fmt --manifest-path Cargo.toml --all
cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
cargo clippy --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --lib -- -D warnings
cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --examples
```

修复 GPUI scope 内的 warnings。只有 intentional compatibility 或 diagnostic code 才使
用局部 `#[expect(..., reason = "...")]`。

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
