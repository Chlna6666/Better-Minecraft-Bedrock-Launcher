use std::collections::HashSet;

use crate::http::proxy::get_client_for_proxy;

use super::install::{LEVILAMINA_PACKAGE, MAX_RESOLVED_PACKAGES, PRELOADER_PACKAGE};
use super::installation_state::InstalledPackage;
use super::lip::{
    PackageId, PackageManifest, PackageVariant, PlacementKind, go_proxy_path, parse_version,
    version_matches,
};

#[derive(Clone, Debug)]
pub(super) struct PendingPackage {
    pub id: PackageId,
    pub requirement: String,
    pub explicit_version: Option<String>,
    pub explicit: bool,
}

#[derive(Clone, Debug)]
pub(super) struct ResolvedPackage {
    pub id: PackageId,
    pub manifest: PackageManifest,
    pub variant: PackageVariant,
    pub dependencies: Vec<PackageId>,
    pub explicit: bool,
    pub disposition: PackageDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PackageDisposition {
    Install,
    Reuse,
}

pub(super) async fn resolve_packages(
    root: PendingPackage,
    game_version: &str,
    installed_packages: &[InstalledPackage],
) -> Result<Vec<ResolvedPackage>, String> {
    let support = super::fetch_support_database().await?;
    let mut pending = vec![root];
    let mut resolved = Vec::<ResolvedPackage>::new();
    while let Some(package) = pending.pop() {
        if is_ignored_dependency(&package.id.path) {
            continue;
        }
        if resolved.len() >= MAX_RESOLVED_PACKAGES {
            return Err("Lip 依赖数量超过安全上限".to_string());
        }
        if let Some(existing) = resolved.iter_mut().find(|item| item.id == package.id) {
            if !version_matches(&existing.manifest.version, &package.requirement) {
                return Err(format!(
                    "Lip 依赖版本冲突: {} {} 与 {}",
                    package.id.display(),
                    existing.manifest.version,
                    package.requirement
                ));
            }
            existing.explicit |= package.explicit;
            if package.explicit {
                existing.disposition = PackageDisposition::Install;
            }
            continue;
        }

        let reusable_version =
            reusable_package_version(&package, installed_packages, game_version, &support);
        let (version, disposition) = if let Some(version) = reusable_version {
            (version.to_owned(), PackageDisposition::Reuse)
        } else {
            (
                select_package_version(&package, game_version, &support).await?,
                PackageDisposition::Install,
            )
        };
        let manifest = fetch_manifest(&package.id.path, &version).await?;
        manifest.validate()?;
        let mut variant = manifest
            .select_variant(&package.id.variant)
            .or_else(|error| {
                if package.explicit
                    && package.id.variant == "client"
                    && !package.id.path.eq_ignore_ascii_case(LEVILAMINA_PACKAGE)
                {
                    manifest.select_variant("")
                } else {
                    Err(error)
                }
            })?;
        if package.id.path.eq_ignore_ascii_case(PRELOADER_PACKAGE) {
            redirect_preloader(&mut variant);
        }
        let dependencies = variant
            .dependencies
            .iter()
            .filter(|(id, _)| !is_ignored_dependency(id))
            .map(|(id, requirement)| {
                let id = PackageId::parse(id)?;
                pending.push(PendingPackage {
                    id: id.clone(),
                    requirement: requirement.clone(),
                    explicit_version: None,
                    explicit: false,
                });
                Ok(id)
            })
            .collect::<Result<Vec<_>, String>>()?;
        resolved.push(ResolvedPackage {
            id: package.id,
            manifest,
            variant,
            dependencies,
            explicit: package.explicit,
            disposition,
        });
    }
    Ok(resolved)
}

fn reusable_package_version<'a>(
    package: &PendingPackage,
    installed_packages: &'a [InstalledPackage],
    game_version: &str,
    support: &super::LeviLaminaSupportDatabase,
) -> Option<&'a str> {
    if package.explicit {
        return None;
    }
    installed_packages
        .iter()
        .find(|installed| {
            installed.id.path.eq_ignore_ascii_case(&package.id.path)
                && installed.id.variant == package.id.variant
                && version_matches(&installed.version, &package.requirement)
                && (!package.id.path.eq_ignore_ascii_case(LEVILAMINA_PACKAGE)
                    || support.supports_loader(game_version, &installed.version))
        })
        .map(|installed| installed.version.as_str())
}

async fn select_package_version(
    package: &PendingPackage,
    game_version: &str,
    support: &super::LeviLaminaSupportDatabase,
) -> Result<String, String> {
    if let Some(version) = &package.explicit_version {
        if package.id.path.eq_ignore_ascii_case(LEVILAMINA_PACKAGE)
            && !support.supports_loader(game_version, version)
        {
            return Err(format!(
                "游戏版本 {game_version} 不支持 LeviLamina {version}"
            ));
        }
        return Ok(version.clone());
    }

    let candidates = if package.id.path.eq_ignore_ascii_case(LEVILAMINA_PACKAGE) {
        support.loader_versions(game_version)
    } else {
        fetch_available_versions(&package.id.path).await?
    };
    candidates
        .into_iter()
        .filter(|version| version_matches(version, &package.requirement))
        .max_by(|left, right| {
            parse_version(left)
                .cmp(&parse_version(right))
                .then_with(|| left.cmp(right))
        })
        .ok_or_else(|| {
            format!(
                "找不到满足 {} 的 Lip 包版本: {}",
                package.requirement,
                package.id.display()
            )
        })
}

async fn fetch_available_versions(package_path: &str) -> Result<Vec<String>, String> {
    let url = format!(
        "https://proxy.golang.org/{}/@v/list",
        go_proxy_path(package_path)
    );
    let body = fetch_text(&url).await?;
    Ok(body
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('v'))
        .map(|line| {
            line.trim_start_matches('v')
                .trim_end_matches("+incompatible")
                .to_string()
        })
        .collect())
}

async fn fetch_manifest(package_path: &str, version: &str) -> Result<PackageManifest, String> {
    let repository = github_repository(package_path)?;
    let url = format!("https://raw.githubusercontent.com/{repository}/v{version}/tooth.json");
    let body = fetch_text(&url).await?;
    serde_json::from_str(&body)
        .map_err(|error| format!("解析 Lip 清单失败 {package_path}@{version}: {error}"))
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let client = get_client_for_proxy().map_err(|error| error.to_string())?;
    client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Lip 请求失败 {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Lip 请求返回错误 {url}: {error}"))?
        .text()
        .await
        .map_err(|error| format!("读取 Lip 响应失败 {url}: {error}"))
}

pub(super) fn installation_order(
    packages: &[ResolvedPackage],
) -> Result<Vec<&ResolvedPackage>, String> {
    let mut installed = HashSet::<PackageId>::new();
    let mut ordered = Vec::with_capacity(packages.len());
    while ordered.len() < packages.len() {
        let Some(package) = packages.iter().find(|package| {
            !installed.contains(&package.id)
                && package.dependencies.iter().all(|dependency| {
                    installed.contains(dependency)
                        || !packages.iter().any(|candidate| &candidate.id == dependency)
                })
        }) else {
            return Err("Lip 依赖图包含循环".to_string());
        };
        installed.insert(package.id.clone());
        ordered.push(package);
    }
    Ok(ordered)
}

pub(super) fn redirect_preloader(variant: &mut PackageVariant) {
    for asset in &mut variant.assets {
        for placement in &mut asset.placements {
            if placement.kind == PlacementKind::File && placement.src.ends_with("PreLoader.dll") {
                placement.destination = "mods/PreLoader/PreLoader.dll".to_string();
            }
        }
    }
}

pub(super) fn github_repository(package_path: &str) -> Result<&str, String> {
    package_path
        .strip_prefix("github.com/")
        .filter(|repository| repository.split('/').count() == 2)
        .ok_or_else(|| format!("当前仅支持 GitHub Lip 包: {package_path}"))
}

pub(super) fn is_ignored_dependency(package_path: &str) -> bool {
    package_path.eq_ignore_ascii_case("github.com/LiteLDev/PeEditor")
        || package_path.eq_ignore_ascii_case("github.com/LiteLDev/bds")
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
