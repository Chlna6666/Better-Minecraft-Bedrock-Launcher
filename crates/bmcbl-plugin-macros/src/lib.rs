use proc_macro::TokenStream;
use quote::quote;
use std::path::PathBuf;
use toml::Value;

#[proc_macro_attribute]
pub fn bmcbl_plugin(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::ItemImpl);
    let self_ty = input.self_ty.clone();
    let options = syn::parse_macro_input!(attr as MacroOptions);
    if options.pack {
        return syn::Error::new_spanned(
            input,
            "#[bmcbl_plugin(pack)] has been removed; use build.rs with \
             bmcbl_plugin_api::pack::auto_pack_from_build_script()",
        )
        .to_compile_error()
        .into();
    }

    let expanded = quote! {
        #input

        bmcbl_plugin_api::export_plugin!(#self_ty);
    };
    expanded.into()
}

#[proc_macro]
pub fn plugin_metadata(input: TokenStream) -> TokenStream {
    if !input.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "plugin_metadata! does not accept arguments",
        )
        .to_compile_error()
        .into();
    }

    match read_plugin_metadata() {
        Ok(metadata) => metadata.to_tokens().into(),
        Err(error) => syn::Error::new(proc_macro2::Span::call_site(), error)
            .to_compile_error()
            .into(),
    }
}

struct MacroOptions {
    pack: bool,
}

impl syn::parse::Parse for MacroOptions {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut options = Self { pack: false };
        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            match ident.to_string().as_str() {
                "pack" => options.pack = true,
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unsupported bmcbl_plugin option `{other}`"),
                    ));
                }
            }

            if input.peek(syn::Token![,]) {
                let _: syn::Token![,] = input.parse()?;
            }
        }
        Ok(options)
    }
}

struct PluginMetadataModel {
    id: String,
    name: String,
    version: String,
    authors: Vec<String>,
    description: String,
    website: String,
    license: String,
    tags: Vec<String>,
    capabilities: Vec<String>,
}

impl PluginMetadataModel {
    fn to_tokens(&self) -> proc_macro2::TokenStream {
        let id = &self.id;
        let name = &self.name;
        let version = &self.version;
        let description = &self.description;
        let website = &self.website;
        let license = &self.license;
        let authors = self.authors.iter();
        let tags = self.tags.iter();
        let capabilities = self.capabilities.iter();

        quote! {
            ::bmcbl_plugin_api::PluginMetadata {
                id: #id,
                name: #name,
                version: #version,
                authors: &[#(#authors),*],
                description: #description,
                website: #website,
                license: #license,
                tags: &[#(#tags),*],
                capabilities: &[#(#capabilities),*],
            }
        }
    }
}

fn plugin_manifest_dir() -> Result<PathBuf, String> {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map_err(|error| format!("read CARGO_MANIFEST_DIR failed: {error}"))
}

fn read_plugin_metadata() -> Result<PluginMetadataModel, String> {
    let manifest_dir = plugin_manifest_dir()?;
    let manifest_path = manifest_dir.join("Cargo.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("read {} failed: {error}", manifest_path.display()))?;
    let manifest = manifest_text
        .parse::<Value>()
        .map_err(|error| format!("parse {} failed: {error}", manifest_path.display()))?;

    let package = manifest
        .get("package")
        .and_then(Value::as_table)
        .ok_or_else(|| "Cargo.toml is missing [package]".to_string())?;
    let metadata = package
        .get("metadata")
        .and_then(Value::as_table)
        .and_then(|metadata| metadata.get("bmcbl-plugin"))
        .and_then(Value::as_table)
        .ok_or_else(|| {
            "Cargo.toml is missing [package.metadata.bmcbl-plugin] for plugin metadata".to_string()
        })?;

    Ok(PluginMetadataModel {
        id: required_string(metadata, "id")?,
        name: optional_string(metadata, "name")
            .or_else(|| optional_string(package, "name"))
            .ok_or_else(|| "Cargo.toml is missing package.name".to_string())?,
        version: optional_string(metadata, "version")
            .or_else(|| optional_string(package, "version"))
            .ok_or_else(|| "Cargo.toml is missing package.version".to_string())?,
        authors: string_array(metadata, "authors")
            .or_else(|| string_array(package, "authors"))
            .unwrap_or_default(),
        description: optional_string(metadata, "description")
            .or_else(|| optional_string(package, "description"))
            .unwrap_or_default(),
        website: optional_string(metadata, "website").unwrap_or_default(),
        license: optional_string(metadata, "license")
            .or_else(|| optional_string(package, "license"))
            .unwrap_or_default(),
        tags: string_array(metadata, "tags").unwrap_or_default(),
        capabilities: string_array(metadata, "capabilities").ok_or_else(|| {
            "metadata.bmcbl-plugin.capabilities must be a string array".to_string()
        })?,
    })
}

fn required_string(table: &toml::map::Map<String, Value>, key: &str) -> Result<String, String> {
    optional_string(table, key)
        .ok_or_else(|| format!("metadata.bmcbl-plugin.{key} must be a string"))
}

fn optional_string(table: &toml::map::Map<String, Value>, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn string_array(table: &toml::map::Map<String, Value>, key: &str) -> Option<Vec<String>> {
    table.get(key).and_then(Value::as_array).map(|values| {
        values
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect()
    })
}
