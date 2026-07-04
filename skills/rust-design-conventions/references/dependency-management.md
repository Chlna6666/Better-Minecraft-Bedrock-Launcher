# 依赖管理与供应链

> 依赖是 Rust 项目的双刃剑：复用生态避免重复造轮子，但每个依赖都是供应链上的信任节点——漏洞、许可证冲突、重复版本、停止维护都会传染到你的项目。生产级 Rust 项目必须有系统的依赖治理：版本锁定、漏洞扫描、许可证合规、MSRV 政策。

## 1. 依赖治理的核心维度

```
依赖风险四象限：
┌─────────────────┬──────────────────┐
│  漏洞（CVE）     │  许可证不兼容      │  → 必须扫描拦截
├─────────────────┼──────────────────┤
│  重复版本        │  停止维护         │  → 需主动监控
└─────────────────┴──────────────────┘

治理工具链：
- Cargo.lock      → 版本锁定（可复现构建）
- cargo audit     → 漏洞扫描（RustSec Advisory）
- cargo deny      → 综合（漏洞 + 许可证 + 来源 + 重复版本）
- cargo outdated  → 过时检测
- cargo bloat     → 体积分析
```

## 2. 版本指定策略

### 2.1 SemVer 范围

```toml
[dependencies]
# ✓ 推荐：用 ^（默认）允许 MINOR/PATCH 升级
serde = "1.0"          # 等价 ^1.0.0：>=1.0.0, <2.0.0
tokio = "1"            # 等价 ^1：>=1.0.0, <2.0.0

# ⚠️ 严格：用 = 精确锁定（一般不推荐，除非有兼容性问题）
serde = "=1.0.193"

# ⚠️ 范围：用 >、<、>=、<=
serde = ">=1.0, <1.5"

# ⚠️ 波浪号：允许 PATCH
serde = "~1.0"         # >=1.0.0, <1.1.0

# Git
my_lib = { git = "https://github.com/org/repo" }
my_lib = { git = "...", branch = "dev" }
my_lib = { git = "...", tag = "v1.0" }
my_lib = { git = "...", rev = "abc123" }  # 最严格

# 本地路径（开发/monorepo）
my_lib = { path = "../my_lib" }
```

### 2.2 默认策略：宽松指定 + Cargo.lock 锁定

```toml
# Cargo.toml：宽松指定（允许生态升级）
[dependencies]
serde = "1"

# Cargo.lock：精确锁定（实际构建用此版本）
# [[package]]
# name = "serde"
# version = "1.0.193"
```

**关键约定：**
- **库（lib）**：不提交 `Cargo.lock`（让下游决定版本）
- **二进制（bin）**：提交 `Cargo.lock`（保证可复现构建）

```bash
# 库项目 .gitignore
echo "Cargo.lock" >> .gitignore

# 二进制项目：保留 Cargo.lock
git add Cargo.lock
```

### 2.3 MSRV（最低支持 Rust 版本）

```toml
[package]
rust-version = "1.75"  # 声明 MSRV
```

**策略：**
- 库的 MSRV 至少支持 N-2 到 N-3 个 stable 版本（生态惯例）
- 提升 MSRV 是 **MINOR breaking**（影响下游），需评估
- 用 `#[cfg(version)]` 条件使用新特性

```rust
// 用条件编译兼容不同 Rust 版本
#[cfg(version("1.80"))]
fn use_new_api() { /* 1.80+ 的新 API */ }

#[cfg(not(version("1.80")))]
fn use_new_api() { /* 旧版本回退 */ }
```

## 3. 漏洞扫描

### 3.1 cargo-audit（RustSec）

```bash
cargo install cargo-audit
cargo audit
# 扫描 Cargo.lock 中所有依赖，对比 RustSec Advisory Database
# 报告已知 CVE 和未维护的 crate
```

**输出示例：**
```
Crate:     openssl
Version:   0.10.40
Title:     openssl XXE vulnerability
Date:      2022-09-13
ID:        RUSTSEC-2022-0057
Solution:  Upgrade to >= 0.10.42

Crate:     chrono
Version:   0.4.19
Warning:   unmaintained
Solution:  Upgrade to >= 0.4.20
```

### 3.2 CI 集成

```yaml
# .github/workflows/audit.yml
name: Security Audit
on:
  schedule:
    - cron: '0 0 * * *'  # 每日扫描
  push:
    paths: ['Cargo.lock']

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rustsec/audit-check@v2.0.0
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
```

**策略：**
- 每日定时扫描（新 CVE 随时可能发布）
- Cargo.lock 变更时触发
- 发现高危漏洞 → CI 失败 + 通知

### 3.3 漏洞响应流程

```
发现漏洞 →
  评估影响（是否影响你的使用路径）→
    影响大 → 立即升级到修复版本
    影响小 → 记录到风险登记册，下个迭代处理
  升级后 → 回归测试 → 验证修复 → 合并
```

## 4. cargo-deny（综合检查）

比 cargo-audit 更全面，一个工具检查四类问题：

### 4.1 配置

```toml
# deny.toml
[advisories]
# 漏洞检查
db-urls = ["https://github.com/rustsec/advisory-db"]
vulnerability = "deny"          # 有 CVE → 失败
unmaintained = "warn"           # 未维护 → 警告
yanked = "deny"                 # 已撤回 → 失败
notice = "warn"
ignore = [
    # "RUSTSEC-2020-0015",  # 已知但暂不处理（注明原因）
]

[licenses]
# 许可证检查
allow = [
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
]
confidence-threshold = 0.8
deny = [
    "GPL-2.0",     # 传染性，避免在闭源项目用
    "GPL-3.0",
    "AGPL-3.0",
]

[bans]
# 重复版本与禁用 crate 检查
multiple-versions = "warn"      # 同一 crate 多版本 → 警告
wildcards = "deny"              # 禁止通配符版本
highlight = "all"
deny = [
    # 禁止特定 crate（团队规范）
    { name = "openssl", use-instead = "rustls" },
    { name = "chrono", use-instead = "time" },  # 若团队标准化用 time
]

[sources]
# 来源限制
unknown-registry = "deny"       # 禁止非 crates.io 的 registry
unknown-git = "deny"            # 禁止未知 git 源
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```

### 4.2 运行

```bash
cargo install cargo-deny
cargo deny check                # 全部检查
cargo deny check advisories     # 仅漏洞
cargo deny check licenses       # 仅许可证
cargo deny check bans           # 仅重复/禁用
cargo deny check sources        # 仅来源
```

### 4.3 CI 集成

```yaml
- uses: EmbarkStudios/cargo-deny-action@v2
  with:
    command: check all
```

## 5. 重复依赖处理

### 5.1 检测

```bash
cargo tree -d          # 显示被多次引入的依赖
# 输出示例：
# chrono v0.4.19
#   └── my_lib v0.1.0
# chrono v0.4.31
#   └── other_lib v0.2.0
```

### 5.2 危害

- 二进制体积膨胀（多个版本都编进去）
- 编译时间增加
- 类型不兼容（`my_lib::chrono::DateTime` ≠ `other_lib::chrono::DateTime`）

### 5.3 解决策略

```toml
# 1. 统一版本（用 [patch] 强制）
[patch.crates-io]
chrono = { version = "0.4.31" }

# 2. 升级依赖到兼容版本
# 若 A 用 0.4.19，B 用 0.4.31 → 升级 A 到支持 0.4.31 的版本

# 3. 检查是否真需要两版本（可能某个依赖卡在老版本）
cargo tree -i chrono   # 反向查看谁依赖 chrono
```

## 6. 许可证合规

### 6.1 常见许可证兼容性

| 许可证 | 类型 | 能否闭源使用 | 注意 |
|--------|------|------------|------|
| MIT | 宽松 | ✓ | 保留版权声明 |
| Apache-2.0 | 宽松 | ✓ | 含专利授权 |
| BSD-2/3-Clause | 宽松 | ✓ | 保留版权 |
| ISC | 宽松 | ✓ | 简化版 MIT/BSD |
| MPL-2.0 | 弱传染 | ✓（按文件） | 修改的文件须开源 |
| LGPL | 弱传染 | ✓（动态链接） | 静态链接复杂 |
| GPL-2/3.0 | 传染 | ✗ | 衍生作品须 GPL |
| AGPL-3.0 | 强传染 | ✗ | 网络服务也须开源 |

### 6.2 闭源项目策略

```toml
# deny.toml
[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC"]
deny = ["GPL-2.0", "GPL-3.0", "AGPL-3.0", "LGPL-2.0", "LGPL-3.0"]
```

### 6.3 许可证声明收集

```bash
# 生成所有依赖的许可证声明（发布合规）
cargo install cargo-about
cargo about generate about.hbs > LICENSES.html
# 生成的 HTML 含所有依赖的许可证全文，随产品分发
```

## 7. 依赖体积分析

```bash
# 分析二进制中各 crate 占比
cargo install cargo-bloat
cargo bloat --release
# 输出示例：
# File  text    Data  Name
# 45.3% serde   12.1% tokio
# 8.2%  reqwest 5.1%  sqlx

# 按 crate 分组
cargo bloat --release --crates

# 找未使用代码（dead code）
cargo bloat --release --time
```

**策略：** 发现占比异常的依赖 → 评估是否真需要 → 用更轻量的替代。

## 8. 依赖选型准则

新增依赖前评估：

| 维度 | 检查项 |
|------|--------|
| **必要性** | 能否用 std/自己写 100 行解决？ |
| **维护活跃度** | 最近 commit 时间、issue 响应速度 |
| **流行度** | downloads、GitHub stars、被多少项目依赖 |
| **质量** | 文档完整性、测试覆盖率、是否有 unsafe |
| **安全** | 是否有已知 CVE、是否依赖 unsafe 多 |
| **体积** | 编译时间、二进制体积贡献 |
| **许可证** | 与项目许可证兼容 |
| **替代品** | 是否有更轻量的等价方案 |

### 常见依赖的轻量替代

```toml
# HTTP 客户端：按需选
reqwest = "0.12"     # 全功能，大
ureq = "2"           # 轻量同步，小
attohttpc = "0.27"   # 更小

# JSON：按需选
serde_json = "1"     # 通用
simd-json = "0.13"   # SIMD 加速（大）
json = "0.12"        # 极简（无依赖）

# 时间
chrono = "0.4"       # 全功能（但有时区争议）
time = "0.3"         # 现代、更安全

# 正则
regex = "1"          # 全功能，大
regex-lite = "0.1"   # 轻量版
```

## 9. 工具链版本管理

### 9.1 rust-toolchain.toml

```toml
# 项目根目录 rust-toolchain.toml
[toolchain]
channel = "1.75.0"            # 固定 Rust 版本
components = ["rustfmt", "clippy", "rust-src"]
targets = ["wasm32-unknown-unknown"]
profile = "minimal"
```

**作用：** 进入项目目录自动切换到指定版本，保证团队/CI 一致。

### 9.2 rustup 策略

```bash
rustup install 1.75.0
rustup default stable
rustup component add rustfmt clippy
```

## 10. 依赖管理检查清单

### 版本
- [ ] 默认宽松指定（`^`），用 Cargo.lock 锁定
- [ ] 库不提交 Cargo.lock，二进制提交
- [ ] 声明 MSRV（`rust-version`）
- [ ] 用 `rust-toolchain.toml` 固定工具链

### 安全
- [ ] CI 跑 `cargo audit`（每日 + Cargo.lock 变更触发）
- [ ] CI 跑 `cargo deny check advisories`
- [ ] yanked crate 检查
- [ ] 漏洞响应流程明确

### 许可证
- [ ] `cargo deny check licenses` 通过
- [ ] allow/deny 列表符合项目需求（闭源严格 deny GPL）
- [ ] 发布前 `cargo about` 生成 LICENSES.html

### 体积与质量
- [ ] `cargo tree -d` 检查重复版本
- [ ] `cargo bloat` 分析体积贡献
- [ ] 新增依赖前评估八维度
- [ ] 优先用轻量替代

### 维护
- [ ] `cargo outdated` 定期检查过时依赖
- [ ] 监控关键依赖的维护状态
- [ ] 依赖升级有回归测试
- [ ] 重大依赖升级单独 PR（便于回滚）
