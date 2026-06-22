fn main() {
    if let Err(error) = bmcbl_plugin_api::pack::auto_pack_from_build_script() {
        println!("cargo:warning=BMCBL plugin auto-pack failed: {error:#}");
        std::process::exit(1);
    }
}
