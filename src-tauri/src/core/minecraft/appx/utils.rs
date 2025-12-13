//! UWP 脱离沙盒运行并支持多开
//!
//! 本实现参考了 GPLv3 许可的 C# 项目
//! mc-w10-version-launcher[](https://github.com/QYCottage/mc-w10-version-launcher/blob/master/MCLauncher/ManifestHelper.cs)
//! 和 【UWP】修改清单脱离沙盒运行[](https://www.cnblogs.com/wherewhere/p/18171253)
//!
//! 原始 C# 项目采用 GPLv3 许可，本项目使用 Rust 实现，采用 GPLv3 许可
//!
//! https://github.com/MicrosoftDocs/windows-dev-docs/blob/docs/uwp/launch-resume/multi-instance-uwp.md

use futures_util::TryFutureExt;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use tracing::debug;
use xmltree::{AttributeMap, Element, EmitterConfig, Namespace, XMLNode};

const SCCD_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<CustomCapabilityDescriptor xmlns="http://schemas.microsoft.com/appx/2018/sccd" xmlns:s="http://schemas.microsoft.com/appx/2018/sccd">
  <CustomCapabilities>
    <CustomCapability Name="Microsoft.coreAppActivation_8wekyb3d8bbwe"></CustomCapability>
  </CustomCapabilities>
  <AuthorizedEntities AllowAny="true"/>
  <Catalog>FFFF</Catalog>
</CustomCapabilityDescriptor>
"#;

/// 写入 SCCD 文件
pub fn write_sccd(dir: &Path) -> io::Result<()> {
    let path = dir.join("CustomCapability.SCCD");
    fs::write(&path, strip_bom(SCCD_XML).as_bytes())?;
    Ok(())
}

/// 去除 UTF-8 BOM
fn strip_bom(s: &str) -> &str {
    const BOM: &str = "\u{feff}";
    s.strip_prefix(BOM).unwrap_or(s)
}

fn has_xmlns_prefix(attrs: &AttributeMap<String, String>, prefix: &str) -> bool {
    let key = format!("xmlns:{}", prefix);
    attrs.contains_key(&key)
}

/// 补丁清单文件以支持 UWP 多开和脱离沙盒运行
pub fn patch_manifest(dir: &Path) -> io::Result<bool> {
    let manifest_path = dir.join("AppxManifest.xml");
    if !manifest_path.exists() {
        return Ok(false);
    }

    // 1. 读取并去除 BOM
    let mut xml_str = String::new();
    File::open(&manifest_path)?.read_to_string(&mut xml_str)?;
    let xml_str = strip_bom(&xml_str);

    // 2. 解析为 XML 树，根元素即 <Package>
    let mut pkg =
        Element::parse(xml_str.as_bytes()).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // 3. 在 pkg.namespaces 中补全 xmlns 前缀，避免重复
    let ns = pkg.namespaces.get_or_insert_with(Namespace::empty);

    // 添加用于多开的 desktop4 命名空间
    if !ns.0.contains_key("desktop4") {
        ns.0.insert(
            "desktop4".to_string(),
            "http://schemas.microsoft.com/appx/manifest/desktop/windows10/4".to_string(),
        );
    }
    // 保留原有的命名空间
    if !ns.0.contains_key("uap4") {
        ns.0.insert(
            "uap4".to_string(),
            "http://schemas.microsoft.com/appx/manifest/uap/windows10/4".to_string(),
        );
    }
    if !ns.0.contains_key("rescap") {
        ns.0.insert(
            "rescap".to_string(),
            "http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities".to_string(),
        );
    }
    if !ns.0.contains_key("uap10") {
        ns.0.insert(
            "uap10".to_string(),
            "http://schemas.microsoft.com/appx/manifest/uap/windows10/10".to_string(),
        );
    }

    // 合并 IgnorableNamespaces 属性，加入 desktop4
    let required = ["uap", "uap4", "uap10", "rescap", "desktop4"];
    pkg.attributes
        .entry("IgnorableNamespaces".into())
        .and_modify(|v| {
            let mut parts: HashSet<_> = v.split_whitespace().collect();
            for &p in &required {
                parts.insert(p);
            }
            *v = parts.into_iter().collect::<Vec<_>>().join(" ");
        })
        .or_insert_with(|| required.join(" "));

    // 4. 更新 <Applications>，添加多开支持
    if let Some(apps) = pkg.get_mut_child("Applications") {
        // 移除多余属性
        apps.attributes.remove("uap10:TrustLevel");
        // 确保每个 Application 节点都有 TrustLevel 和 SupportsMultipleInstances
        for child in apps.children.iter_mut().filter_map(|n| match n {
            XMLNode::Element(e) => Some(e),
            _ => None,
        }) {
            if child.name == "Application" {
                // 添加脱离沙盒的 TrustLevel
                child
                    .attributes
                    .entry("uap10:TrustLevel".into())
                    .or_insert_with(|| "mediumIL".into());
                // 添加多开支持
                child.attributes.insert(
                    "desktop4:SupportsMultipleInstances".to_string(),
                    "true".to_string(),
                );
            }
        }
    }

    // 5. 重建 <Capabilities>，顺序：[Capability*] → [rescap:Capability*] → [uap4:CustomCapability*] → [DeviceCapability*]
    if let Some(caps) = pkg.get_mut_child("Capabilities") {
        // 1) 把原 children 一次性拿出
        let old = std::mem::take(&mut caps.children);

        // 2) 分类到四组
        let mut group1 = Vec::new(); // <Capability>
        let mut group3 = Vec::new(); // <rescap:Capability>
        let mut group4 = Vec::new(); // <uap4:CustomCapability>
        let mut group2 = Vec::new(); // <DeviceCapability>
        for node in old {
            match &node {
                XMLNode::Element(e) if e.name == "Capability" => {
                    group1.push(node.clone());
                }
                XMLNode::Element(e) if e.name == "rescap:Capability" => {
                    group3.push(node.clone());
                }
                XMLNode::Element(e) if e.name == "uap4:CustomCapability" => {
                    group4.push(node.clone());
                }
                XMLNode::Element(e) if e.name == "DeviceCapability" => {
                    group2.push(node.clone());
                }
                _ => {
                    group1.push(node.clone());
                }
            }
        }

        // 3) 确保 runFullTrust 和 uap4 自定义存在
        let ensure = |grp: &mut Vec<XMLNode>, tag: &str, name: &str| {
            if !grp.iter().any(|n| match n {
                XMLNode::Element(e) => {
                    e.name == tag && e.attributes.get("Name") == Some(&name.to_string())
                }
                _ => false,
            }) {
                let mut e = Element::new(tag);
                e.attributes.insert("Name".into(), name.into());
                grp.push(XMLNode::Element(e));
            }
        };
        ensure(&mut group3, "rescap:Capability", "runFullTrust");
        ensure(
            &mut group4,
            "uap4:CustomCapability",
            "Microsoft.coreAppActivation_8wekyb3d8bbwe",
        );

        // 4) 清空并按新顺序拼回
        caps.children.clear();
        caps.children.extend(group1);
        caps.children.extend(group3);
        caps.children.extend(group4);
        caps.children.extend(group2);
    } else {
        // 若一开始没有 <Capabilities>，则按同一顺序创建
        let mut caps = Element::new("Capabilities");
        // group3: runFullTrust
        caps.children.push(XMLNode::Element({
            let mut e = Element::new("rescap:Capability");
            e.attributes.insert("Name".into(), "runFullTrust".into());
            e
        }));
        // group4: uap4 自定义
        caps.children.push(XMLNode::Element({
            let mut e = Element::new("uap4:CustomCapability");
            e.attributes.insert(
                "Name".into(),
                "Microsoft.coreAppActivation_8wekyb3d8bbwe".into(),
            );
            e
        }));
        pkg.children.push(XMLNode::Element(caps));
    }

    // 6. 清理自闭合节点
    for node in pkg.children.iter_mut() {
        if let XMLNode::Element(ref mut elem) = node {
            if matches!(
                elem.name.as_str(),
                "Identity" | "PhoneIdentity" | "TargetDeviceFamily" | "PackageDependency"
            ) {
                elem.children.clear();
            }
        }
    }

    // 7. 序列化输出并格式化（统一 CRLF 换行和自闭合）
    let mut out = Vec::new();
    let cfg = EmitterConfig::new()
        .perform_indent(true)
        .write_document_declaration(true)
        .normalize_empty_elements(true)
        .line_separator("\r\n");
    pkg.write_with_config(&mut out, cfg)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    fs::write(&manifest_path, out)?;

    // 8. 写入 SCCD
    write_sccd(dir)?;
    Ok(true)
}

/// 获取包信息
pub fn get_package_info(
    app_user_model_id: &str,
) -> windows::core::Result<Option<(String, String, String)>> {
    match windows::ApplicationModel::AppInfo::GetFromAppUserModelId(&app_user_model_id.into()) {
        Ok(app_info) => match app_info.Package() {
            Ok(package) => {
                let version = if let Ok(version) = package.Id().and_then(|id| id.Version()) {
                    Some(format!(
                        "{}.{}.{}.{}",
                        version.Major, version.Minor, version.Build, version.Revision
                    ))
                } else {
                    None
                };

                let package_family_name =
                    if let Ok(package_family_name) = package.Id().and_then(|id| id.FamilyName()) {
                        Some(package_family_name)
                    } else {
                        return Err(windows::core::Error::from(io::Error::new(
                            io::ErrorKind::Other,
                            "无法获取包家族名称",
                        )));
                    };

                let package_full_name =
                    if let Ok(package_full_name) = package.Id().and_then(|id| id.FullName()) {
                        Some(package_full_name.to_string())
                    } else {
                        return Err(windows::core::Error::from(io::Error::new(
                            io::ErrorKind::Other,
                            "无法获取包全名",
                        )));
                    };

                Ok(Some((
                    version.unwrap(),
                    package_family_name.unwrap().to_string(),
                    package_full_name.unwrap(),
                )))
            }
            Err(err) => Err(err.into()),
        },
        Err(err) => Err(err.into()),
    }
}

/// 异步获取清单中的 Identity 信息
pub async fn get_manifest_identity(appx_path: &str) -> Result<(String, String), String> {
    let manifest_path = Path::new(appx_path).join("AppxManifest.xml");
    debug!("Manifest 路径: {}", manifest_path.display());

    // 异步读取（确保使用 tokio::fs）
    let xml = tokio::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("无法打开/读取文件 {}: {}", manifest_path.display(), e))
        .await?;

    // 找到第一个 <Identity ...> 或 <Identity/...>
    let start_idx = match xml.find("<Identity") {
        Some(i) => i,
        None => return Err("未找到 <Identity> 节点".to_string()),
    };
    // 找到标签结束符号 '>'（包括自闭合 "/>" 情况）
    let rest = &xml[start_idx..];
    let end_rel = rest.find('>').ok_or("无法定位 Identity 标签结束")?;
    let tag = &rest[..=end_rel]; // 包含 '>'

    // 简单属性提取器（支持双引号或单引号）
    fn extract_attr(tag: &str, key: &str) -> Option<String> {
        let needle = format!("{}=", key);
        let pos = tag.find(&needle)?;
        let after = &tag[pos + needle.len()..].trim_start();
        let mut chars = after.chars();
        let first = chars.next()?;
        if first == '"' || first == '\'' {
            let quote = first;
            let mut val = String::new();
            for c in chars {
                if c == quote {
                    return Some(val);
                }
                val.push(c);
            }
            None
        } else {
            // 非引号情况（罕见）——读到空白或'>'或'/'
            let val: String = after
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
                .collect();
            if val.is_empty() {
                None
            } else {
                Some(val)
            }
        }
    }

    let name = extract_attr(tag, "Name");
    let version = extract_attr(tag, "Version");

    match (name, version) {
        (Some(n), Some(v)) => {
            debug!("解析结果 => Name: {}, Version: {}", n, v);
            Ok((n, v))
        }
        _ => Err("未找到 Identity 的 Name 或 Version".to_string()),
    }
}
