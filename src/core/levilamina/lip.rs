use std::collections::HashMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

const LIP_FORMAT_VERSION: u32 = 3;
const LIP_FORMAT_UUID: &str = "289f771f-2c9a-4d73-9f3f-8492495a924d";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PackageManifest {
    pub format_version: u32,
    pub format_uuid: String,
    pub tooth: String,
    pub version: String,
    #[serde(default)]
    pub info: PackageInfo,
    #[serde(default)]
    pub variants: Vec<PackageVariant>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct PackageInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct PackageVariant {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    #[serde(default)]
    pub assets: Vec<PackageAsset>,
    #[serde(default)]
    pub preserve_files: Vec<String>,
    #[serde(default)]
    pub remove_files: Vec<String>,
    #[serde(default)]
    pub scripts: PackageScripts,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PackageAsset {
    #[serde(rename = "type")]
    pub kind: AssetKind,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub placements: Vec<AssetPlacement>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum AssetKind {
    #[serde(rename = "self")]
    Self_,
    Tar,
    Tgz,
    Uncompressed,
    Zip,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct AssetPlacement {
    #[serde(rename = "type")]
    pub kind: PlacementKind,
    pub src: String,
    #[serde(rename = "dest")]
    pub destination: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum PlacementKind {
    File,
    Dir,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct PackageScripts {
    #[serde(default)]
    pub pre_install: Vec<String>,
    #[serde(default)]
    pub install: Vec<String>,
    #[serde(default)]
    pub post_install: Vec<String>,
    #[serde(default)]
    pub pre_uninstall: Vec<String>,
    #[serde(default)]
    pub uninstall: Vec<String>,
    #[serde(default)]
    pub post_uninstall: Vec<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct PackageId {
    pub path: String,
    pub variant: String,
}

impl PackageId {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (path, variant) = raw.split_once('#').unwrap_or((raw, ""));
        if path.trim().is_empty() || path.contains(['\\', '?']) || variant.contains('#') {
            return Err(format!("无效 Lip 包标识: {raw}"));
        }
        Ok(Self {
            path: path.to_string(),
            variant: variant.to_string(),
        })
    }

    #[must_use]
    pub fn display(&self) -> String {
        if self.variant.is_empty() {
            self.path.clone()
        } else {
            format!("{}#{}", self.path, self.variant)
        }
    }
}

impl PackageManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != LIP_FORMAT_VERSION || self.format_uuid != LIP_FORMAT_UUID {
            return Err(format!(
                "不支持的 Lip 清单格式: version={}, uuid={}",
                self.format_version, self.format_uuid
            ));
        }
        if self.tooth.trim().is_empty() || self.version.trim().is_empty() {
            return Err("Lip 清单缺少 tooth 或 version".to_string());
        }
        Ok(())
    }

    pub fn select_variant(&self, label: &str) -> Result<PackageVariant, String> {
        let matches = self
            .variants
            .iter()
            .filter(|variant| variant.label == label && platform_matches(&variant.platform))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Err(format!(
                "包 {}@{} 不支持 client/win-x64 变体 '{}'",
                self.tooth, self.version, label
            ));
        }

        let mut selected = PackageVariant {
            label: label.to_string(),
            platform: "win-x64".to_string(),
            ..PackageVariant::default()
        };
        for variant in matches {
            selected.dependencies.extend(variant.dependencies.clone());
            selected.assets.extend(variant.assets.clone());
            selected
                .preserve_files
                .extend(variant.preserve_files.clone());
            selected.remove_files.extend(variant.remove_files.clone());
            merge_scripts(&mut selected.scripts, &variant.scripts);
        }
        Ok(selected)
    }
}

fn merge_scripts(target: &mut PackageScripts, source: &PackageScripts) {
    target.pre_install.extend(source.pre_install.clone());
    target.install.extend(source.install.clone());
    target.post_install.extend(source.post_install.clone());
    target.pre_uninstall.extend(source.pre_uninstall.clone());
    target.uninstall.extend(source.uninstall.clone());
    target.post_uninstall.extend(source.post_uninstall.clone());
}

fn platform_matches(platform: &str) -> bool {
    platform.is_empty()
        || platform.eq_ignore_ascii_case("win-x64")
        || platform.eq_ignore_ascii_case("win-*")
        || platform == "*"
}

pub(super) fn render_template(value: &str, manifest: &PackageManifest) -> String {
    value
        .replace("{{tooth}}", &manifest.tooth)
        .replace("{{version}}", &manifest.version)
        .replace("{{info.name}}", &manifest.info.name)
        .replace("{{info.description}}", &manifest.info.description)
}

pub(super) fn version_matches(version: &str, requirement: &str) -> bool {
    let Some(version) = parse_version(version) else {
        return false;
    };
    requirement.split("||").any(|alternative| {
        let normalized = normalize_requirement(alternative);
        normalized == "*"
            || VersionReq::parse(&normalized)
                .is_ok_and(|version_requirement| version_requirement.matches(&version))
    })
}

pub(super) fn parse_version(version: &str) -> Option<Version> {
    let clean = version
        .trim()
        .trim_start_matches('v')
        .split_once("+incompatible")
        .map_or_else(|| version.trim().trim_start_matches('v'), |(base, _)| base);
    Version::parse(clean).ok()
}

fn normalize_requirement(requirement: &str) -> String {
    let trimmed = requirement.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return "*".to_string();
    }
    if trimmed.contains(',') || !trimmed.contains(' ') {
        let has_range_syntax = trimmed.contains(['*', '^', '~', '<', '>', '=']);
        return if has_range_syntax {
            trimmed.to_string()
        } else {
            format!("={trimmed}")
        };
    }
    trimmed.split_whitespace().collect::<Vec<_>>().join(", ")
}

pub(super) fn go_proxy_path(package_path: &str) -> String {
    let mut escaped = String::with_capacity(package_path.len());
    for character in package_path.chars() {
        if character.is_ascii_uppercase() {
            escaped.push('!');
            escaped.push(character.to_ascii_lowercase());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

pub(super) fn go_proxy_version(version: &str) -> String {
    parse_version(version).map_or_else(
        || version.to_string(),
        |parsed| {
            if parsed.major >= 2 && !version.ends_with("+incompatible") {
                format!("{version}+incompatible")
            } else {
                version.to_string()
            }
        },
    )
}

#[cfg(test)]
#[path = "lip_tests.rs"]
mod tests;
