use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum CustomGeometryBoneRole {
    Static,
    Head,
    Body,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CustomGeometryBoneBinding {
    pub(super) role: CustomGeometryBoneRole,
    pub(super) pivot: [f32; 3],
}

impl CustomGeometryBoneBinding {
    pub(super) const fn static_bone() -> Self {
        Self {
            role: CustomGeometryBoneRole::Static,
            pivot: [0.0, 0.0, 0.0],
        }
    }
}

pub(super) struct CustomGeometryBoneDescriptor {
    pub(super) name: String,
    pub(super) parent: Option<String>,
    pub(super) pivot: [f32; 3],
}

pub(super) fn custom_geometry_bone_bindings(
    descriptors: impl IntoIterator<Item = CustomGeometryBoneDescriptor>,
) -> HashMap<String, CustomGeometryBoneBinding> {
    let descriptors = descriptors
        .into_iter()
        .map(|descriptor| (descriptor.name.clone(), descriptor))
        .collect::<HashMap<_, _>>();
    let names = descriptors.keys().cloned().collect::<Vec<_>>();
    let mut bindings = HashMap::with_capacity(descriptors.len());

    for name in names {
        let mut visiting = HashSet::new();
        resolve_bone_binding(&name, &descriptors, &mut bindings, &mut visiting);
    }

    bindings
}

fn resolve_bone_binding(
    name: &str,
    descriptors: &HashMap<String, CustomGeometryBoneDescriptor>,
    bindings: &mut HashMap<String, CustomGeometryBoneBinding>,
    visiting: &mut HashSet<String>,
) -> CustomGeometryBoneBinding {
    if let Some(binding) = bindings.get(name).copied() {
        return binding;
    }
    if !visiting.insert(name.to_string()) {
        return CustomGeometryBoneBinding::static_bone();
    }

    let binding = descriptors
        .get(name)
        .map(|descriptor| {
            if let Some(role) = exact_bone_role(&descriptor.name) {
                CustomGeometryBoneBinding {
                    role,
                    pivot: descriptor.pivot,
                }
            } else if let Some(parent) = descriptor.parent.as_deref() {
                resolve_bone_binding(parent, descriptors, bindings, visiting)
            } else if let Some(role) = fuzzy_bone_role(&descriptor.name) {
                CustomGeometryBoneBinding {
                    role,
                    pivot: descriptor.pivot,
                }
            } else {
                CustomGeometryBoneBinding::static_bone()
            }
        })
        .unwrap_or_else(CustomGeometryBoneBinding::static_bone);

    visiting.remove(name);
    bindings.insert(name.to_string(), binding);
    binding
}

fn exact_bone_role(name: &str) -> Option<CustomGeometryBoneRole> {
    let normalized = normalized_bone_name(name);
    match normalized.as_str() {
        "head" | "neck" => Some(CustomGeometryBoneRole::Head),
        "body" | "torso" | "waist" => Some(CustomGeometryBoneRole::Body),
        "leftarm" | "armleft" | "larm" | "arml" => Some(CustomGeometryBoneRole::LeftArm),
        "rightarm" | "armright" | "rarm" | "armr" => Some(CustomGeometryBoneRole::RightArm),
        "leftleg" | "legleft" | "lleg" | "legl" => Some(CustomGeometryBoneRole::LeftLeg),
        "rightleg" | "legright" | "rleg" | "legr" => Some(CustomGeometryBoneRole::RightLeg),
        _ => None,
    }
}

fn fuzzy_bone_role(name: &str) -> Option<CustomGeometryBoneRole> {
    let normalized = normalized_bone_name(name);
    match normalized.as_str() {
        _ if normalized.contains("left") && normalized.contains("arm") => {
            Some(CustomGeometryBoneRole::LeftArm)
        }
        _ if normalized.contains("right") && normalized.contains("arm") => {
            Some(CustomGeometryBoneRole::RightArm)
        }
        _ if normalized.contains("left") && normalized.contains("leg") => {
            Some(CustomGeometryBoneRole::LeftLeg)
        }
        _ if normalized.contains("right") && normalized.contains("leg") => {
            Some(CustomGeometryBoneRole::RightLeg)
        }
        _ => None,
    }
}

fn normalized_bone_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_bone_inherits_player_limb_binding() {
        let bindings = custom_geometry_bone_bindings([
            CustomGeometryBoneDescriptor {
                name: "leftArm".to_string(),
                parent: Some("body".to_string()),
                pivot: [2.0, 9.0, 0.0],
            },
            CustomGeometryBoneDescriptor {
                name: "leftSleeve".to_string(),
                parent: Some("leftArm".to_string()),
                pivot: [5.0, 6.0, 0.0],
            },
        ]);

        let binding = bindings
            .get("leftSleeve")
            .copied()
            .expect("left sleeve binding should exist");

        assert_eq!(binding.role, CustomGeometryBoneRole::LeftArm);
        assert_eq!(binding.pivot, [2.0, 9.0, 0.0]);
    }
}
