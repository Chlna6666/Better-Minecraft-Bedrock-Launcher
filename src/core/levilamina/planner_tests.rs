use std::collections::HashMap;

use super::*;

fn support_database() -> super::super::LeviLaminaSupportDatabase {
    super::super::LeviLaminaSupportDatabase {
        format_version: 2,
        versions: HashMap::from([("1.26.20.04".to_string(), vec!["26.20.7".to_string()])]),
    }
}

fn levilamina_package(explicit: bool) -> Result<PendingPackage, String> {
    Ok(PendingPackage {
        id: PackageId::parse("github.com/LiteLDev/LeviLamina#client")?,
        requirement: ">=26.20.0 <26.30.0".to_string(),
        explicit_version: None,
        explicit,
    })
}

fn installed_levilamina() -> Result<InstalledPackage, String> {
    Ok(InstalledPackage {
        id: PackageId::parse("github.com/LiteLDev/LeviLamina#client")?,
        version: "26.20.7".to_string(),
    })
}

#[test]
fn compatible_installed_dependency_is_reused() -> Result<(), String> {
    let package = levilamina_package(false)?;
    let installed = [installed_levilamina()?];
    let support = support_database();

    assert_eq!(
        reusable_package_version(&package, &installed, "1.26.20.4", &support),
        Some("26.20.7")
    );
    Ok(())
}

#[test]
fn explicit_install_does_not_reuse_installed_package() -> Result<(), String> {
    let package = levilamina_package(true)?;
    let installed = [installed_levilamina()?];
    let support = support_database();

    assert_eq!(
        reusable_package_version(&package, &installed, "1.26.20.4", &support),
        None
    );
    Ok(())
}
