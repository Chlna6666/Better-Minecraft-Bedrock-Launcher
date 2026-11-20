//! 插件系统性能基准。
//!
//! 运行方式（基准入口只在启用 `bench-support` 特性时存在）：
//!
//! ```text
//! cargo bench --features bench-support --bench plugin_runtime
//! cargo bench --features bench-support --bench plugin_runtime -- --sample-size 10 --measurement-time 1
//! ```
//!
//! 覆盖插件宿主侧的热路径：依赖图拓扑排序、事件级联预算、插件扫描与增量重载，每帧页面/状态
//! 投影，以及真实 Wasm 执行。
//!
//! Wasm 执行基准的夹具是 `examples/plugins/hello-wasm` 以 `wasm32-wasip2` 编译出的真实
//! 组件（Cranelift JIT 加载），运行前先构建：
//!
//! ```text
//! cd examples/plugins/hello-wasm && cargo build --target wasm32-wasip2
//! ```
//!
//! 夹具缺失时 wasm 组自动跳过，其余纯宿主侧基准不受影响。

use bmcbl::bench_support::{
    CURRENT_API_VERSION, DependencyGraph, EventCascade, PluginEvent, PluginEventKind, PluginManifest,
    PluginRegistry, dispatch_event,
};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 每个基准用例的插件规模，用来观察 O(N) 与 O(变化) 的差异。
const PLUGIN_COUNTS: [usize; 3] = [8, 64, 256];

fn manifest_text(id: &str, version: &str) -> String {
    format!(
        r#"
schema_version = 1
id = "{id}"
name = "Bench {id}"
version = "{version}"
api_version = "{api_version}"
entry = "plugin.wasm"
capabilities = ["ui.page", "event.global"]
"#,
        api_version = CURRENT_API_VERSION,
    )
}

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("bmcbl-plugin-bench-{label}-{nonce}"))
}

/// 写入 `count` 个插件目录并返回它们的清单。
fn write_plugins(plugins_dir: &Path, count: usize, version: &str) -> Vec<PluginManifest> {
    fs::create_dir_all(plugins_dir).expect("create plugins dir");
    let mut manifests = Vec::with_capacity(count);
    for index in 0..count {
        let id = format!("bench-{index:03}");
        let dir = plugins_dir.join(&id);
        fs::create_dir_all(&dir).expect("create plugin dir");
        fs::write(dir.join("plugin.toml"), manifest_text(&id, version)).expect("write manifest");
        fs::write(dir.join("plugin.wasm"), b"bench fixture payload").expect("write wasm stub");
        manifests.push(PluginManifest::load_from_dir(&dir).expect("load bench manifest"));
    }
    manifests
}

fn bench_dependency_graph(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("plugin/dependency_graph");
    for count in PLUGIN_COUNTS {
        let mut graph = DependencyGraph::new();
        for index in 0..count {
            let dependencies = if index == 0 {
                BTreeSet::new()
            } else {
                BTreeSet::from([format!("bench-{:03}", index - 1)])
            };
            graph.add_plugin(format!("bench-{index:03}"), dependencies);
        }
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |bencher, _| {
            bencher.iter(|| black_box(graph.compute_load_order().expect("valid graph")));
        });
    }
    group.finish();
}

fn bench_event_cascade(criterion: &mut Criterion) {
    criterion.bench_function("plugin/event_cascade/try_deliver", |bencher| {
        bencher.iter(|| {
            let mut cascade = EventCascade::new();
            let mut delivered = 0_u32;
            while cascade.try_deliver(black_box("alpha"), black_box("download-finished")) {
                delivered += 1;
            }
            black_box(delivered)
        });
    });
}

/// 归属分析用：目录指纹扫描的原始系统调用成本（`read_dir` + 每个文件的元数据）。
///
/// 与 `plugin/registry_reload/unchanged` 对照可以判断每次重载的固定开销里有多少来自文件系统。
fn bench_source_scan(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("plugin/source_scan");
    for count in PLUGIN_COUNTS {
        let plugins_dir = temp_root(&format!("scan-{count}"));
        let manifests = write_plugins(&plugins_dir, count, "0.1.0");
        let dirs = manifests
            .iter()
            .map(|manifest| manifest.root_dir.clone())
            .collect::<Vec<_>>();
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |bencher, _| {
            bencher.iter(|| {
                let mut bytes = 0_u64;
                for dir in &dirs {
                    for entry in fs::read_dir(dir).expect("read bench dir") {
                        let entry = entry.expect("read bench entry");
                        bytes += entry.metadata().expect("bench entry metadata").len();
                    }
                }
                black_box(bytes)
            });
        });
        let _ = fs::remove_dir_all(&plugins_dir);
    }
    group.finish();
}

fn bench_registry_reload(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("plugin/registry_reload");
    for count in PLUGIN_COUNTS {
        let plugins_dir = temp_root(&format!("plugins-{count}"));
        let cache_dir = temp_root(&format!("cache-{count}"));
        let package_cache_dir = temp_root(&format!("packages-{count}"));
        let manifests = write_plugins(&plugins_dir, count, "0.1.0");
        let mut registry = PluginRegistry::new(
            plugins_dir.clone(),
            cache_dir.clone(),
            package_cache_dir.clone(),
        );
        registry
            .reload_manifests(manifests.clone())
            .expect("initial bench reload");

        group.throughput(Throughput::Elements(count as u64));

        // 未变化：全部插件命中准备缓存并复用实例。
        group.bench_with_input(
            BenchmarkId::new("unchanged", count),
            &count,
            |bencher, _| {
                bencher.iter_batched(
                    || manifests.clone(),
                    |batch| black_box(registry.reload_manifests(batch).expect("bench reload")),
                    BatchSize::SmallInput,
                );
            },
        );

        // 单插件变化：只有它需要重建，其余仍然复用。
        let mut changed = manifests.clone();
        changed[0].version = "0.2.0".to_string();
        group.bench_with_input(
            BenchmarkId::new("one_changed", count),
            &count,
            |bencher, _| {
                bencher.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let started = Instant::now();
                        black_box(
                            registry
                                .reload_manifests(changed.clone())
                                .expect("bench reload"),
                        );
                        elapsed += started.elapsed();

                        // Restore the unchanged baseline outside the measured interval so every
                        // sample exercises exactly one changed manifest.
                        registry
                            .reload_manifests(manifests.clone())
                            .expect("reset bench reload");
                    }
                    elapsed
                });
            },
        );

        // Reset the one-changed fixture before measuring the filesystem watcher path.
        registry
            .reload_manifests(manifests.clone())
            .expect("reset bench reload");

        // watcher 路径：重新扫描插件目录、读取清单，再走同一套准备与增量应用。
        group.bench_with_input(
            BenchmarkId::new("reload_all", count),
            &count,
            |bencher, _| {
                bencher.iter(|| black_box(registry.reload_all().expect("bench reload_all")));
            },
        );

        drop(registry);
        let _ = fs::remove_dir_all(&plugins_dir);
        let _ = fs::remove_dir_all(&cache_dir);
        let _ = fs::remove_dir_all(&package_cache_dir);
    }
    group.finish();
}

fn bench_registry_projections(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("plugin/registry_projection");
    for count in PLUGIN_COUNTS {
        let plugins_dir = temp_root(&format!("projection-plugins-{count}"));
        let cache_dir = temp_root(&format!("projection-cache-{count}"));
        let package_cache_dir = temp_root(&format!("projection-packages-{count}"));
        let manifests = write_plugins(&plugins_dir, count, "0.1.0");
        let mut registry = PluginRegistry::new(
            plugins_dir.clone(),
            cache_dir.clone(),
            package_cache_dir.clone(),
        );
        registry
            .reload_manifests(manifests)
            .expect("initial bench reload");

        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::new("tool_pages", count),
            &count,
            |bencher, _| bencher.iter(|| black_box(registry.tool_pages())),
        );
        group.bench_with_input(BenchmarkId::new("statuses", count), &count, |bencher, _| {
            bencher.iter(|| black_box(registry.statuses()));
        });
        group.bench_with_input(
            BenchmarkId::new("memory_report", count),
            &count,
            |bencher, _| bencher.iter(|| black_box(registry.memory_report())),
        );

        drop(registry);
        let _ = fs::remove_dir_all(&plugins_dir);
        let _ = fs::remove_dir_all(&cache_dir);
        let _ = fs::remove_dir_all(&package_cache_dir);
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Wasm 执行夹具
// ---------------------------------------------------------------------------

/// 夹具插件的插件 id。
const FIXTURE_PLUGIN_ID: &str = "bench-fixture";

fn fixture_manifest_text() -> String {
    format!(
        r#"
schema_version = 1
id = "{FIXTURE_PLUGIN_ID}"
name = "Bench Fixture"
version = "0.1.0"
api_version = "{api_version}"
entry = "plugin.wasm"
capabilities = ["ui.page", "ui.hook", "event.global", "toast", "config.read"]
"#,
        api_version = CURRENT_API_VERSION,
    )
}

fn route_changed_event() -> PluginEvent {
    PluginEvent {
        plugin_id: None,
        page_id: None,
        kind: PluginEventKind::RouteChanged {
            path: "/settings".to_string(),
        },
    }
}

/// 定位预编译的夹具组件；`BMCBL_PLUGIN_FIXTURE` 可覆盖。
fn fixture_component_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BMCBL_PLUGIN_FIXTURE") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        "examples/plugins/hello-wasm/target/wasm32-wasip2/release/bmcbl_essentials.wasm",
        "examples/plugins/hello-wasm/target/wasm32-wasip2/debug/bmcbl_essentials.wasm",
    ];
    candidates
        .iter()
        .map(|relative| manifest_dir.join(relative))
        .find(|path| path.is_file())
}

/// 写出夹具插件并加载它；返回 `None` 表示夹具不可用（未编译或加载失败）。
fn load_fixture_plugin() -> Option<(RefCell<PluginRegistry>, Vec<PathBuf>)> {
    let component = match std::fs::read(fixture_component_path()?) {
        Ok(component) => component,
        Err(error) => {
            eprintln!("[bench] 读取夹具组件失败: {error}");
            return None;
        }
    };

    let plugins_dir = temp_root("wasm-plugins");
    let cache_dir = temp_root("wasm-cache");
    let package_cache_dir = temp_root("wasm-packages");
    let plugin_dir = plugins_dir.join(FIXTURE_PLUGIN_ID);
    fs::create_dir_all(&plugin_dir).expect("create fixture plugin dir");
    fs::write(plugin_dir.join("plugin.wasm"), &component).expect("write fixture wasm");
    fs::write(plugin_dir.join("plugin.toml"), fixture_manifest_text())
        .expect("write fixture manifest");

    let manifest = PluginManifest::load_from_dir(&plugin_dir).expect("load fixture manifest");
    let mut registry = PluginRegistry::new(
        plugins_dir.clone(),
        cache_dir.clone(),
        package_cache_dir.clone(),
    );
    registry
        .reload_manifests(vec![manifest])
        .expect("prepare fixture plugin");

    // 先探测一次渲染：夹具组件不可用时给出原因，而不是让基准中途 panic。
    if let Err(error) = registry.render_page(FIXTURE_PLUGIN_ID, "main") {
        eprintln!("[bench] 跳过 plugin/wasm：夹具组件加载失败：{error}");
        return None;
    }

    Some((
        RefCell::new(registry),
        vec![plugins_dir, cache_dir, package_cache_dir],
    ))
}

/// Wasm 执行基准（Cranelift 组件）：页面渲染、事件处理与生命周期。
///
/// 夹具是 hello-wasm 的真实组件：`render_page` 会做一次 config 读取 + 多次翻译调用，
/// 覆盖「渲染 + 宿主调用往返」的真实路径；Cranelift JIT 下解释器吞吐不再构成独立维度，
/// `guest_loop` 系列工作负载随旧 ABI 一并移除。
///
/// 渲染组在计时区内先失效页面缓存再渲染：数字是「失效 + 冷渲染」，并带自检断言防止
/// 退化成渲染缓存查找。
fn bench_wasm_execution(criterion: &mut Criterion) {
    let Some((registry, _dirs)) = load_fixture_plugin() else {
        eprintln!("[bench] 跳过 plugin/wasm：夹具组件不可用（先编译 hello-wasm fixture）");
        return;
    };

    let mut group = criterion.benchmark_group("plugin/wasm");
    group.bench_function("render_page", |bencher| {
        bencher.iter(|| {
            let mut registry = registry.borrow_mut();
            registry.trim_plugin_caches(FIXTURE_PLUGIN_ID);
            black_box(
                registry
                    .render_page(FIXTURE_PLUGIN_ID, "main")
                    .expect("fixture render"),
            )
        });
    });
    group.bench_function("handle_event", |bencher| {
        bencher.iter(|| {
            black_box(dispatch_event(
                &mut registry.borrow_mut(),
                route_changed_event(),
            ))
        });
    });
    group.finish();

    // 生命周期单独一间：休眠后重新加载，度量编译缓存命中 + Store 实例化 + init + 首次渲染。
    let mut group = criterion.benchmark_group("plugin/wasm_lifecycle");
    group.bench_function("instantiate_and_init", |bencher| {
        bencher.iter(|| {
            let mut registry = registry.borrow_mut();
            registry
                .hibernate_plugin(FIXTURE_PLUGIN_ID)
                .expect("hibernate fixture plugin");
            registry.trim_plugin_caches(FIXTURE_PLUGIN_ID);
            black_box(
                registry
                    .render_page(FIXTURE_PLUGIN_ID, "main")
                    .expect("fixture render"),
            )
        });
    });
    group.finish();

    // 自检：失效后连续两次渲染必须返回不同的树，否则渲染组退化成缓存查找。两棵树必须同时
    // 存活，否则上一棵树释放后分配器可能复用同一地址，指针比较会误报。
    {
        let mut registry = registry.borrow_mut();
        registry.trim_plugin_caches(FIXTURE_PLUGIN_ID);
        let first = registry
            .render_page(FIXTURE_PLUGIN_ID, "main")
            .expect("fixture render");
        registry.trim_plugin_caches(FIXTURE_PLUGIN_ID);
        let second = registry
            .render_page(FIXTURE_PLUGIN_ID, "main")
            .expect("fixture render");
        assert_ne!(
            std::sync::Arc::as_ptr(&first),
            std::sync::Arc::as_ptr(&second),
            "plugin/wasm render_page: 失效后仍返回同一棵树，基准会退化为缓存查找"
        );
    }

    for dir in _dirs {
        let _ = fs::remove_dir_all(dir);
    }
}

criterion_group!(
    plugin_benches,
    bench_dependency_graph,
    bench_event_cascade,
    bench_source_scan,
    bench_registry_reload,
    bench_registry_projections,
    bench_wasm_execution
);
criterion_main!(plugin_benches);
