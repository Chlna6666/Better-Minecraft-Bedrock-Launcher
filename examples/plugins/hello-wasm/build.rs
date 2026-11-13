fn main() {
    if let Err(error) = bmcbl_plugin_api::pack::auto_pack() {
        println!("cargo:warning=BMCBL plugin auto-pack failed: {error:#}");
        std::process::exit(1);
    }
}
