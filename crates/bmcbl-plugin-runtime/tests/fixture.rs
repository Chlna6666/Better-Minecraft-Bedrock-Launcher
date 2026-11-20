//! 端到端 fixture 测试：加载一个真实编译的插件组件并跑通全部入口。
//!
//! fixture 由 `examples/plugins/hello-wasm` 以 `wasm32-wasip2` 编译产出；先构建：
//!
//! ```text
//! cargo build --target wasm32-wasip2 -p bmcbl-essentials (在 examples/plugins/hello-wasm 内)
//! ```
//!
//! 然后运行（默认被忽略，避免无 fixture 时失败）：
//!
//! ```text
//! cargo test -p bmcbl_plugin_runtime --test fixture -- --ignored
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use bmcbl_plugin_runtime::{
    Allowlist, AppInfo, PluginCapabilities, PluginEngine, PluginError, PluginExecution,
    PluginHostContext, PluginInitState, PluginLimits, PluginPolicy,
};

const FIXTURE_RELATIVE: &[&str] =
    &["../../examples/plugins/hello-wasm/target/wasm32-wasip2/debug/bmcbl_essentials.wasm"];

fn fixture_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BMCBL_PLUGIN_FIXTURE") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    FIXTURE_RELATIVE
        .iter()
        .map(|relative| manifest_dir.join(relative))
        .find(|path| path.is_file())
}

struct MockContext;

impl PluginHostContext for MockContext {
    fn invoke_service(&self, name: &str, arguments: Vec<u8>) -> Result<String, PluginError> {
        assert_eq!(name, "bedrock-world.query.is-slime-chunk");
        assert_eq!(
            postcard::from_bytes::<(i32, i32)>(&arguments).unwrap(),
            (-2, 7)
        );
        Ok("fixture-job".to_string())
    }

    fn take_service(&self, id: &str) -> Result<Option<Vec<u8>>, PluginError> {
        assert_eq!(id, "fixture-job");
        Ok(Some(
            postcard::to_allocvec(&true).expect("encode fixture result"),
        ))
    }

    fn http_get_text(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _url: &str,
        _ttl_seconds: u32,
        _max_bytes: u32,
    ) -> Result<bmcbl_plugin_runtime::HttpTextResponse, PluginError> {
        Err(PluginError::not_found("mock context has no http"))
    }

    fn read_resource_text(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _path: &str,
    ) -> Result<String, PluginError> {
        Err(PluginError::not_found("mock context has no resources"))
    }

    fn read_resource_bytes(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _path: &str,
    ) -> Result<Vec<u8>, PluginError> {
        Err(PluginError::not_found("mock context has no resources"))
    }

    fn write_config(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _text: &str,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn task_create(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::TaskCreateRequest,
    ) -> Result<String, PluginError> {
        Err(PluginError::not_found("mock context has no tasks"))
    }

    fn task_update(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::TaskUpdateRequest,
    ) -> Result<(), PluginError> {
        Err(PluginError::not_found("mock context has no tasks"))
    }

    fn task_finish(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::TaskFinishRequest,
    ) -> Result<(), PluginError> {
        Err(PluginError::not_found("mock context has no tasks"))
    }

    fn task_register_definition(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::TaskDefinitionRequest,
    ) -> Result<(), PluginError> {
        Err(PluginError::not_found("mock context has no tasks"))
    }

    fn task_create_typed(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::TaskCreateRequest,
        _definition: &str,
    ) -> Result<String, PluginError> {
        Err(PluginError::not_found("mock context has no tasks"))
    }

    fn service_call(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::ServiceCallRequest,
    ) -> Result<Vec<u8>, PluginError> {
        Err(PluginError::not_found("mock context has no services"))
    }

    fn app_info(&self) -> AppInfo {
        AppInfo {
            version: "0.0.0-test".to_string(),
            build_info: "fixture".to_string(),
            api_version: "1.0.0".to_string(),
        }
    }

    fn download_resource(
        &self,
        _info: bmcbl_plugin_runtime::PluginCallInfo<'_>,
        _request: bmcbl_plugin_runtime::DownloadRequest,
    ) -> Result<String, PluginError> {
        Err(PluginError::not_found("mock context has no downloads"))
    }
}

fn fixture_policy() -> PluginPolicy {
    PluginPolicy {
        plugin_id: "bmcbl-essentials".to_string(),
        capabilities: PluginCapabilities {
            ui_page: true,
            ui_window: true,
            ui_hook: true,
            event_global: true,
            toast: true,
            config_read: true,
            ..PluginCapabilities::default()
        },
        limits: PluginLimits::default(),
        network_allow: Allowlist::default(),
        resource_allow: Allowlist::default(),
        external_allow: Allowlist::default(),
    }
}

fn instantiate_fixture() -> (PluginEngine, PluginExecution) {
    let wasm_path = fixture_path().expect("build hello-wasm for wasm32-wasip2 first");
    instantiate_tool_fixture(&wasm_path, fixture_policy())
}

fn instantiate_tool_fixture(
    wasm_path: &std::path::Path,
    policy: PluginPolicy,
) -> (PluginEngine, PluginExecution) {
    let wasm = std::fs::read(wasm_path).expect("read fixture wasm");

    let engine = PluginEngine::new().expect("engine");
    let artifact = engine
        .load_component(&wasm)
        .expect("component should compile");

    let execution = PluginExecution::instantiate(
        &engine,
        &artifact,
        policy,
        Arc::new(MockContext),
        PluginInitState {
            locale: "en-US".to_string(),
            translations: BTreeMap::new(),
            theme: bmcbl_plugin_runtime::ThemeSnapshot::light_default(),
            clipboard_text: None,
            storage_snapshot: Ok(BTreeMap::new()),
            config_text: Ok("key = \"value\"\n".to_string()),
        },
    )
    .expect("instantiate");
    (engine, execution)
}

#[test]
#[ignore = "requires a real standalone tool component"]
fn fixture_external_tool_registers_and_renders() {
    let path = std::env::var_os("BMCBL_TOOL_FIXTURE").expect("standalone tool fixture");
    let plugin_id = std::env::var("BMCBL_TOOL_FIXTURE_ID").expect("fixture plugin ID");
    let mut policy = fixture_policy();
    policy.plugin_id = plugin_id.clone();
    let (_engine, mut execution) = instantiate_tool_fixture(std::path::Path::new(&path), policy);
    let registrations = execution
        .init(bmcbl_plugin_runtime::PluginContext {
            plugin_id: plugin_id.clone(),
            api_version: "1.0.0".into(),
        })
        .expect("initialize tool component");
    let page_id = registrations
        .into_iter()
        .find_map(|registration| match registration {
            bmcbl_plugin_runtime::Registration::Page(page) if page.tool.is_some() => {
                Some(page.page_id)
            }
            _ => None,
        })
        .expect("tool registration");
    let tree = execution
        .render_page(bmcbl_plugin_runtime::PageRenderRequest { plugin_id, page_id })
        .expect("render external tool UI");
    tree.validate().expect("render tree validation");
    execution
        .shutdown(bmcbl_plugin_runtime::ShutdownReason::Unload)
        .expect("shutdown tool component");
}

fn init_context() -> bmcbl_plugin_runtime::PluginContext {
    bmcbl_plugin_runtime::PluginContext {
        plugin_id: "bmcbl-essentials".to_string(),
        api_version: "1.0.0".to_string(),
    }
}

#[test]
#[ignore = "requires the compiled hello-wasm fixture (see module docs)"]
fn fixture_plugin_runs_all_entries() {
    let (_engine, mut execution) = instantiate_fixture();
    let effects = execution.take_effects();
    assert!(effects.is_empty(), "instantiate must not emit effects");

    // init：hello-wasm 注册 page + injection + subscription
    let registrations = execution.init(init_context()).expect("init should succeed");
    assert_eq!(registrations.len(), 3, "page + injection + subscription");
    assert!(registrations.iter().any(|registration| matches!(
        registration, bmcbl_plugin_runtime::Registration::Page(page) if page.tool.is_some()
    )));

    execution
        .handle_event(bmcbl_plugin_runtime::PluginEvent {
            plugin_id: Some("bmcbl-essentials".to_string()),
            page_id: None,
            kind: bmcbl_plugin_runtime::PluginEventKind::JobReady(
                bmcbl_plugin_runtime::JobReadyEvent {
                    job_id: "fixture-job".to_string(),
                },
            ),
        })
        .expect("handle the completed SDK job");
    assert!(execution.take_effects().iter().any(|effect| matches!(
        effect, bmcbl_plugin_runtime::PluginEffect::Log { message, .. } if message.contains("host world query: true")
    )));

    // render_page：返回一个可用的 ViewTree
    let tree = execution
        .render_page(bmcbl_plugin_runtime::PageRenderRequest {
            plugin_id: "bmcbl-essentials".to_string(),
            page_id: "main".to_string(),
        })
        .expect("render_page should succeed");
    assert!(tree.nodes.len() > 3, "page should render several nodes");
    tree.validate().expect("valid generated WIT tree");

    // handle_event：未知 action 也应 Ok
    execution
        .handle_event(bmcbl_plugin_runtime::PluginEvent {
            plugin_id: Some("bmcbl-essentials".to_string()),
            page_id: Some("main".to_string()),
            kind: bmcbl_plugin_runtime::PluginEventKind::Action(
                bmcbl_plugin_runtime::ActionEvent {
                    action_id: "unknown-action".to_string(),
                    value: None,
                },
            ),
        })
        .expect("handle_event should succeed");

    // shutdown
    execution
        .shutdown(bmcbl_plugin_runtime::ShutdownReason::Unload)
        .expect("shutdown should succeed");
}

#[test]
#[ignore = "requires the compiled hello-wasm fixture (see module docs)"]
fn fixture_artifact_cache_roundtrip() {
    let wasm_path = fixture_path().expect("build hello-wasm for wasm32-wasip2 first");
    let wasm = std::fs::read(wasm_path).expect("read fixture wasm");
    let cache_dir = tempfile::tempdir().expect("temporary cache directory");
    let first = PluginEngine::with_cache(Some(cache_dir.path()))
        .expect("first engine")
        .load_component(&wasm)
        .expect("compile");
    assert!(
        cache_dir
            .path()
            .read_dir()
            .expect("read cache")
            .next()
            .is_some()
    );
    let second = PluginEngine::with_cache(Some(cache_dir.path()))
        .expect("second engine")
        .load_component(&wasm)
        .expect("load");
    assert_eq!(first.sha256, second.sha256);
}
