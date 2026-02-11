use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub manifest_version: u32,
    pub entry: String,
    #[serde(default = "default_plugin_type")]
    pub r#type: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub version: String,
    pub authors: Vec<PluginAuthor>,
    pub dependencies: Option<Vec<PluginDependency>>,

    #[serde(skip_deserializing, default)]
    pub root_path: String,
}

fn default_plugin_type() -> String {
    "javascript".to_string()
}