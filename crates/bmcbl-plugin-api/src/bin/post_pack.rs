use anyhow::Result;

fn main() -> Result<()> {
    bmcbl_plugin_api::pack::run_post_pack_worker_from_env()
}
