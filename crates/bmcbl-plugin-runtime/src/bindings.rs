//! 从 `crates/bmcbl-plugin-api/wit/plugin.wit` 构建期生成的宿主绑定。
//!
//! 契约归 `bmcbl_plugin_api` crate 所有；本 crate 通过相对路径引入同一份 WIT，
//! 保证宿主与 guest 的类型由同一契约生成。

wasmtime::component::bindgen!({
    path: "../bmcbl-plugin-api/wit",
    world: "plugin-world",
});
