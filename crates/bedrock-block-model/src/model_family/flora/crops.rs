use crate::model_family::ModelFamily;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if matches!(name, "wheat" | "carrots" | "potatoes" | "beetroot") || name.ends_with("_crop") {
        Some(ModelFamily::CrossPlant)
    } else {
        None
    }
}
