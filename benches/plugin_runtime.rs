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
//! Wasm 执行基准的夹具是按 BMCBL 插件 ABI 在基准启动时用 `wasm-encoder` 现场生成的模块，
//! 因此不需要 wasm 工具链，也不依赖仓库里的构建产物：入口函数、线性内存、数据段响应与宿主
//! 调用都在生成器里显式构造。

use bmcbl::bench_support::{
    DependencyGraph, EventCascade, PluginEvent, PluginEventKind, PluginManifest, PluginRegistry,
    ROUTE_CHANGED_EVENT, dispatch_event,
};
use bmcbl_plugin_api as abi;
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function as WasmFunction, FunctionSection, ImportSection, Instruction, MemorySection,
    MemoryType, Module, TypeSection, ValType,
};

/// 每个基准用例的插件规模，用来观察 O(N) 与 O(变化) 的差异。
const PLUGIN_COUNTS: [usize; 3] = [8, 64, 256];

fn manifest_text(id: &str, version: &str) -> String {
    format!(
        r#"
schema_version = 2
id = "{id}"
name = "Bench {id}"
version = "{version}"
api_version = "{api_version}"
entry = "plugin.wasm"
capabilities = ["ui.page", "event.global"]
"#,
        api_version = bmcbl_plugin_api::API_VERSION,
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
                bencher.iter_batched(
                    || changed.clone(),
                    |batch| black_box(registry.reload_manifests(batch).expect("bench reload")),
                    BatchSize::SmallInput,
                );
            },
        );

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
            BenchmarkId::new("navigation_pages", count),
            &count,
            |bencher, _| bencher.iter(|| black_box(registry.navigation_pages())),
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

/// 夹具数据段基址（响应字节与宿主调用请求都放在这里）。
const FIXTURE_DATA_BASE: u32 = 1024;
/// 宿主写入入口请求的缓冲地址；夹具不需要真正的分配器。
const FIXTURE_REQUEST_BUFFER: i32 = 4096;
/// 夹具执行宿主调用时使用的响应缓冲地址。
const FIXTURE_HOST_CALL_RESPONSE: i32 = 8192;
/// 宿主调用导出函数的功能索引（唯一导入为索引 0）。
const APP_CALL_FUNCTION: u32 = 0;

/// 夹具在 `render_page` / `handle_event` 里执行的插件侧工作量。
#[derive(Clone, Copy, Debug)]
enum FixtureWorkload {
    /// 直接返回预置响应，用于度量宿主侧固定开销。
    ResponseOnly,
    /// 先做 `count` 次宿主调用（`CurrentUnixMs`，无能力要求）再返回响应。
    HostCalls(u32),
    /// 先跑 `count` 圈解释器循环（每圈 11 条指令）再返回响应。
    GuestLoop(u32),
}

/// postcard 编码的响应与请求字节，以及它们在数据段中的位置。
struct FixtureData {
    bytes: Vec<u8>,
    init: (u32, usize),
    view_tree: (u32, usize),
    injection: (u32, usize),
    unit: (u32, usize),
    app_request: (u32, usize),
}

fn push_blob(bytes: &mut Vec<u8>, blob: &[u8]) -> (u32, usize) {
    let offset = FIXTURE_DATA_BASE + bytes.len() as u32;
    bytes.extend_from_slice(blob);
    (offset, blob.len())
}

fn push_encoded<T: serde::Serialize>(bytes: &mut Vec<u8>, value: &T) -> (u32, usize) {
    let blob = postcard::to_allocvec(value).expect("encode fixture blob");
    push_blob(bytes, &blob)
}

/// 夹具返回的视图树：一个容器 + 8 个文本 + 1 个按钮，接近真实插件的页面规模。
fn fixture_view_tree() -> abi::ViewTree {
    let style = abi::default_style();
    let mut nodes = vec![abi::ViewNode::Container(abi::ContainerNode {
        style,
        children: Vec::new(),
    })];
    let mut children = Vec::new();
    for index in 0..8 {
        children.push(nodes.len() as u32);
        nodes.push(abi::ViewNode::Text(abi::TextNode {
            text: format!("Bench row {index}").into(),
            style,
        }));
    }
    children.push(nodes.len() as u32);
    nodes.push(abi::ViewNode::Button(abi::ButtonNode {
        label: "Open".into(),
        action_id: "bench-open".into(),
        action_value: None,
        style,
    }));
    nodes[0] = abi::ViewNode::Container(abi::ContainerNode { style, children });
    abi::ViewTree { root: 0, nodes }
}

impl FixtureData {
    fn build() -> Self {
        let init = abi::AbiResult::Ok(vec![abi::Registration::Subscription(
            abi::EventSubscription {
                event: ROUTE_CHANGED_EVENT.to_string(),
            },
        )]);
        let injection = abi::AbiResult::Ok(Some(fixture_view_tree()));
        let unit = abi::AbiResult::Ok(());
        let app_request = abi::AppRequest::CurrentUnixMs;

        let mut bytes = Vec::new();
        let init = push_encoded(&mut bytes, &init);
        let view_tree = push_encoded(&mut bytes, &abi::AbiResult::Ok(fixture_view_tree()));
        let injection = push_encoded(&mut bytes, &injection);
        let unit = push_encoded(&mut bytes, &unit);
        let app_request = push_encoded(&mut bytes, &app_request);
        Self {
            bytes,
            init,
            view_tree,
            injection,
            unit,
            app_request,
        }
    }
}

/// ABI 约定：入口函数返回 `(ptr << 32) | len`。
fn packed_response((offset, len): (u32, usize)) -> i64 {
    ((u64::from(offset) << 32) | len as u64) as i64
}

const NO_LOCALS: [ValType; 0] = [];

/// 往入口函数体里追加插件侧工作量。
///
/// 入口参数占用局部变量 0、1，夹具自己的局部变量从 2 开始：2 是 i32 计数器，3 是 i64 累加器。
fn append_workload(function: &mut WasmFunction, workload: FixtureWorkload, data: &FixtureData) {
    const COUNTER: u32 = 2;
    const ACCUMULATOR: u32 = 3;
    let (count, app_calls) = match workload {
        FixtureWorkload::ResponseOnly => return,
        FixtureWorkload::HostCalls(count) => (count, true),
        FixtureWorkload::GuestLoop(count) => (count, false),
    };

    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(COUNTER));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(COUNTER));
    function.instruction(&Instruction::I32Const(count as i32));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    if app_calls {
        function.instruction(&Instruction::I32Const(abi::AppOp::CurrentUnixMs.code()));
        function.instruction(&Instruction::I32Const(data.app_request.0 as i32));
        function.instruction(&Instruction::I32Const(data.app_request.1 as i32));
        function.instruction(&Instruction::I32Const(FIXTURE_HOST_CALL_RESPONSE));
        function.instruction(&Instruction::I32Const(64));
        function.instruction(&Instruction::Call(APP_CALL_FUNCTION));
        function.instruction(&Instruction::Drop);
    } else {
        function.instruction(&Instruction::LocalGet(ACCUMULATOR));
        function.instruction(&Instruction::I64Const(1));
        function.instruction(&Instruction::I64Add);
        function.instruction(&Instruction::LocalSet(ACCUMULATOR));
    }
    function.instruction(&Instruction::LocalGet(COUNTER));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(COUNTER));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
}

/// 生成一个满足 BMCBL 插件 ABI 的最小 wasm 模块。
///
/// 导入恰好一个宿主函数、导出线性内存与七个入口、数据段里放好各入口的 postcard 响应，
/// 因此它可以通过 `validate_module_abi` 并走完整的宿主调用路径，而不需要 wasm 工具链。
fn fixture_module(workload: FixtureWorkload, data: &FixtureData) -> Vec<u8> {
    const APP_CALL_TYPE: u32 = 0;
    const ALLOC_TYPE: u32 = 1;
    const DEALLOC_TYPE: u32 = 2;
    const ENTRY_TYPE: u32 = 3;

    let mut types = TypeSection::new();
    types.ty().function([ValType::I32; 5], [ValType::I64]);
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32], []);
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I64]);

    let mut imports = ImportSection::new();
    imports.import(
        "bmcbl",
        "bmcbl_host_call",
        EntityType::Function(APP_CALL_TYPE),
    );

    let mut functions = FunctionSection::new();
    functions.function(ALLOC_TYPE);
    functions.function(DEALLOC_TYPE);
    for _ in 0..5 {
        functions.function(ENTRY_TYPE);
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 17,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("bmcbl_alloc", ExportKind::Func, 1);
    exports.export("bmcbl_dealloc", ExportKind::Func, 2);
    exports.export("bmcbl_init", ExportKind::Func, 3);
    exports.export("bmcbl_handle_event", ExportKind::Func, 4);
    exports.export("bmcbl_render_page", ExportKind::Func, 5);
    exports.export("bmcbl_render_injection", ExportKind::Func, 6);
    exports.export("bmcbl_shutdown", ExportKind::Func, 7);

    let mut code = CodeSection::new();
    let mut alloc = WasmFunction::new_with_locals_types(NO_LOCALS);
    alloc.instruction(&Instruction::I32Const(FIXTURE_REQUEST_BUFFER));
    alloc.instruction(&Instruction::End);
    code.function(&alloc);

    let mut dealloc = WasmFunction::new_with_locals_types(NO_LOCALS);
    dealloc.instruction(&Instruction::End);
    code.function(&dealloc);

    let mut init = WasmFunction::new_with_locals_types(NO_LOCALS);
    init.instruction(&Instruction::I64Const(packed_response(data.init)));
    init.instruction(&Instruction::End);
    code.function(&init);

    for (response, with_workload) in [(data.unit, true), (data.view_tree, true)] {
        let mut entry = WasmFunction::new_with_locals_types([ValType::I32, ValType::I64]);
        if with_workload {
            append_workload(&mut entry, workload, data);
        }
        entry.instruction(&Instruction::I64Const(packed_response(response)));
        entry.instruction(&Instruction::End);
        code.function(&entry);
    }

    let mut render_injection = WasmFunction::new_with_locals_types(NO_LOCALS);
    render_injection.instruction(&Instruction::I64Const(packed_response(data.injection)));
    render_injection.instruction(&Instruction::End);
    code.function(&render_injection);

    let mut shutdown = WasmFunction::new_with_locals_types(NO_LOCALS);
    shutdown.instruction(&Instruction::I64Const(packed_response(data.unit)));
    shutdown.instruction(&Instruction::End);
    code.function(&shutdown);

    let mut segments = DataSection::new();
    segments.active(
        0,
        &ConstExpr::i32_const(FIXTURE_DATA_BASE as i32),
        data.bytes.iter().copied(),
    );

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&memories);
    module.section(&exports);
    module.section(&code);
    module.section(&segments);
    module.finish()
}

fn fixture_manifest_text() -> String {
    format!(
        r#"
schema_version = 2
id = "{FIXTURE_PLUGIN_ID}"
name = "Bench Fixture"
version = "0.1.0"
api_version = "{api_version}"
entry = "plugin.wasm"
capabilities = ["ui.page", "event.global"]
"#,
        api_version = bmcbl_plugin_api::API_VERSION,
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

/// 写出夹具插件并加载它；返回 `None` 表示夹具不可用（生成或校验失败）。
fn load_fixture_plugin(
    workload: FixtureWorkload,
) -> Option<(RefCell<PluginRegistry>, Vec<PathBuf>)> {
    let data = FixtureData::build();
    let module = fixture_module(workload, &data);

    let plugins_dir = temp_root("wasm-plugins");
    let cache_dir = temp_root("wasm-cache");
    let package_cache_dir = temp_root("wasm-packages");
    let plugin_dir = plugins_dir.join(FIXTURE_PLUGIN_ID);
    fs::create_dir_all(&plugin_dir).expect("create fixture plugin dir");
    fs::write(plugin_dir.join("plugin.wasm"), &module).expect("write fixture wasm");
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

    // 先探测一次渲染：夹具模块不符合 ABI 时给出原因，而不是让基准中途 panic。
    if let Err(error) = registry.render_page(FIXTURE_PLUGIN_ID, "main") {
        eprintln!("[bench] 跳过 plugin/wasm：夹具插件渲染失败：{error}");
        return None;
    }

    Some((
        RefCell::new(registry),
        vec![plugins_dir, cache_dir, package_cache_dir],
    ))
}

/// Wasm 执行基准：实例化加 init、页面渲染、事件处理。
///
/// 夹具是按 ABI 现场生成的 wasm 模块，因此不需要 wasm 工具链或仓库里的构建产物：
/// `response_only` 度量启动器侧固定开销，`app_calls_8` 度量 8 次启动器往返，`guest_loop_100k`
/// 度量解释器吞吐。
///
/// 渲染组在计时区内先失效页面缓存再渲染：criterion 的 `iter_batched` setup 在同一批次里只
/// 调用一次，把失效放在 setup 里会让绝大多数迭代命中渲染缓存（实测 95ns 级缓存查找，而真实
/// 冷渲染是 µs 级）。因此这里的数字是「失效 + 冷渲染」，并带一条自检断言防止再次退化。
fn bench_wasm_execution(criterion: &mut Criterion) {
    let workloads = [
        ("response_only", FixtureWorkload::ResponseOnly),
        ("app_calls_8", FixtureWorkload::HostCalls(8)),
        ("guest_loop_100k", FixtureWorkload::GuestLoop(100_000)),
    ];
    let mut fixtures = Vec::new();
    for (label, workload) in workloads {
        if let Some(fixture) = load_fixture_plugin(workload) {
            fixtures.push((label, fixture));
        }
    }
    if fixtures.is_empty() {
        eprintln!("[bench] 跳过 plugin/wasm：夹具插件不可用");
        return;
    }

    let mut group = criterion.benchmark_group("plugin/wasm");
    for (label, (registry, _dirs)) in &fixtures {
        group.bench_function(BenchmarkId::new("render_page", *label), |bencher| {
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
        group.bench_function(BenchmarkId::new("handle_event", *label), |bencher| {
            bencher.iter(|| {
                black_box(dispatch_event(
                    &mut registry.borrow_mut(),
                    route_changed_event(),
                ))
            });
        });
    }
    group.finish();

    // 生命周期单独一间：卸载后重新加载，度量 Store 实例化 + `bmcbl_init` + 首次渲染。
    let mut group = criterion.benchmark_group("plugin/wasm_lifecycle");
    if let Some((_, (registry, _dirs))) = fixtures.first() {
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
    }
    group.finish();

    // 自检：失效后连续两次渲染必须返回不同的树，否则渲染组退化成缓存查找。两棵树必须同时存活，
    // 否则上一棵树释放后分配器可能复用同一地址，指针比较会误报。
    if let Some((label, (registry, _dirs))) = fixtures.first() {
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
            "plugin/wasm render_page/{label}: 失效后仍返回同一棵树，基准会退化为缓存查找"
        );
    }

    for (_, (_, dirs)) in fixtures {
        for dir in dirs {
            let _ = fs::remove_dir_all(dir);
        }
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
