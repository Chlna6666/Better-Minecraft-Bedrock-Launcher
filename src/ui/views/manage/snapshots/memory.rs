use super::super::state::{ManageCapePreviewEntry, ManageSkinPreviewEntry};
use super::{ManageAssetEntry, Snapshot};
use gpui::SharedString;

fn strings<'a>(values: impl IntoIterator<Item = Option<&'a SharedString>>) -> usize {
    // Include allocation/refcount overhead and allow for spare string capacity.
    values
        .into_iter()
        .flatten()
        .map(|value| value.len().saturating_mul(2) + 32)
        .sum()
}

fn asset_bytes(asset: &ManageAssetEntry) -> usize {
    let text = strings([
        Some(&asset.key),
        Some(&asset.folder_name),
        Some(&asset.display_name),
        Some(&asset.file_path),
        Some(&asset.open_path),
        asset.detail.as_ref(),
        asset.description.as_ref(),
        asset.icon_path.as_ref(),
        asset.modified_iso.as_ref(),
        asset.modified_label.as_ref(),
        asset.size_label.as_ref(),
        asset.source.as_ref(),
        asset.edition.as_ref(),
        asset.gdk_user.as_ref(),
        asset.mod_type.as_ref(),
        asset.first_skin_full_texture_path.as_ref(),
        asset.first_skin_model_label.as_ref(),
    ]);
    let skins = asset
        .skin_previews
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(skin_bytes)
        .sum::<usize>();
    let capes = asset
        .cape_previews
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(cape_bytes)
        .sum::<usize>();
    size_of_val(asset) + text + skins + capes
}

fn skin_bytes(skin: &ManageSkinPreviewEntry) -> usize {
    size_of_val(skin)
        + strings([
            Some(&skin.display_name),
            Some(&skin.full_texture_path),
            Some(&skin.model_label),
            skin.preview_path.as_ref(),
            skin.geometry_path.as_ref(),
            skin.geometry_identifier.as_ref(),
        ])
}

fn cape_bytes(cape: &ManageCapePreviewEntry) -> usize {
    size_of_val(cape)
        + strings([
            Some(&cape.display_name),
            Some(&cape.texture_path),
            Some(&cape.preview_path),
            cape.elytra_texture_path.as_ref(),
        ])
}

impl Snapshot {
    pub(super) fn estimated_bytes(&self) -> usize {
        64 + match self {
            Self::Assets(entries) => entries.iter().map(asset_bytes).sum::<usize>(),
            Self::Screenshots(entries) => entries
                .iter()
                .map(|entry| {
                    size_of_val(entry)
                        + strings([
                            Some(&entry.key),
                            Some(&entry.image_path),
                            Some(&entry.folder_path),
                            Some(&entry.file_name),
                            entry.capture_time_iso.as_ref(),
                            entry.capture_time_label.as_ref(),
                            entry.modified_iso.as_ref(),
                            entry.modified_label.as_ref(),
                            entry.size_label.as_ref(),
                            entry.gdk_user.as_ref(),
                        ])
                })
                .sum(),
            Self::Servers(entries) => entries
                .iter()
                .map(|entry| {
                    size_of_val(entry)
                        + strings([
                            Some(&entry.key),
                            Some(&entry.name),
                            Some(&entry.address),
                            Some(&entry.file_path),
                        ])
                })
                .sum(),
        }
    }
}
