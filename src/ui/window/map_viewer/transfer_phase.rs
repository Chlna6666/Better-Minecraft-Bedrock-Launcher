use gpui::SharedString;

/// Stable machine-readable phase of a map chunk transfer or export operation.
///
/// The phase drives progress bookkeeping and UI filtering, so producers must
/// use the associated constants instead of localized text. Display names are
/// resolved through [`TransferPhase::label`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct TransferPhase(&'static str);

impl TransferPhase {
    const fn new(code: &'static str) -> Self {
        Self(code)
    }

    pub(super) const fn code(self) -> &'static str {
        self.0
    }

    /// Translation key for the phase display name.
    pub(super) fn label_key(self) -> Option<&'static str> {
        Some(match self.0 {
            "copy_chunks" => "MapViewer.phase.copy_chunks",
            "read_chunks" => "MapViewer.phase.read_chunks",
            "export_image" => "MapViewer.phase.export_image",
            "paste_chunks" => "MapViewer.phase.paste_chunks",
            "generate_obj_text" => "MapViewer.phase.generate_obj_text",
            "serialize_region_pack" => "MapViewer.phase.serialize_region_pack",
            "compress_region_pack" => "MapViewer.phase.compress_region_pack",
            "delete_chunks" => "MapViewer.phase.delete_chunks",
            "write_structure" => "MapViewer.phase.write_structure",
            "read_structure" => "MapViewer.phase.read_structure",
            "encode_image" => "MapViewer.phase.encode_image",
            "paste_structure" => "MapViewer.phase.paste_structure",
            "build_paste_preview" => "MapViewer.phase.build_paste_preview",
            "generate_image" => "MapViewer.phase.generate_image",
            "build_obj_model" => "MapViewer.phase.build_obj_model",
            "stage_history_record" => "MapViewer.phase.stage_history_record",
            "commit_chunk_write" => "MapViewer.phase.commit_chunk_write",
            "export_structure" => "MapViewer.phase.export_structure",
            "export_region_pack" => "MapViewer.phase.export_region_pack",
            "export_obj" => "MapViewer.phase.export_obj",
            "import_structure" => "MapViewer.phase.import_structure",
            "import_region_pack" => "MapViewer.phase.import_region_pack",
            "prepare_paste_chunks" => "MapViewer.phase.prepare_paste_chunks",
            "prepare_transform_chunks" => "MapViewer.phase.prepare_transform_chunks",
            "write_file" => "MapViewer.phase.write_file",
            "write_obj_file" => "MapViewer.phase.write_obj_file",
            "delete_block_entities" => "MapViewer.phase.delete_block_entities",
            "delete_actors" => "MapViewer.phase.delete_actors",
            "reset_chunk" => "MapViewer.phase.reset_chunk",
            "write_transformed_chunks" => "MapViewer.phase.write_transformed_chunks",
            "prepare_structure" => "MapViewer.phase.prepare_structure",
            "undo_history" => "MapViewer.phase.undo_history",
            "redo_history" => "MapViewer.phase.redo_history",
            "restore_history" => "MapViewer.phase.restore_history",
            "capture_snapshot" => "MapViewer.phase.capture_snapshot",
            "save_history" => "MapViewer.phase.save_history",
            "write_history_file" => "MapViewer.phase.write_history_file",
            "apply_history_undo" => "MapViewer.phase.apply_history_undo",
            "apply_history_redo" => "MapViewer.phase.apply_history_redo",
            _ => return None,
        })
    }

    /// Resolves the display name in the active locale.
    pub(super) fn label(self) -> SharedString {
        let Some(key) = self.label_key() else {
            return SharedString::from(self.0.to_owned());
        };
        crate::ui::state::i18n::global_i18n()
            .lookup(key)
            .unwrap_or_else(|| SharedString::from(self.0.to_owned()))
    }

    pub(super) const CAPTURE_SNAPSHOT: Self = Self::new("capture_snapshot");
    pub(super) const SAVE_HISTORY: Self = Self::new("save_history");
    pub(super) const WRITE_HISTORY_FILE: Self = Self::new("write_history_file");
    pub(super) const APPLY_HISTORY_UNDO: Self = Self::new("apply_history_undo");
    pub(super) const APPLY_HISTORY_REDO: Self = Self::new("apply_history_redo");
    pub(super) const COPY_CHUNKS: Self = Self::new("copy_chunks");
    pub(super) const READ_CHUNKS: Self = Self::new("read_chunks");
    pub(super) const EXPORT_IMAGE: Self = Self::new("export_image");
    pub(super) const PASTE_CHUNKS: Self = Self::new("paste_chunks");
    pub(super) const GENERATE_OBJ_TEXT: Self = Self::new("generate_obj_text");
    pub(super) const SERIALIZE_REGION_PACK: Self = Self::new("serialize_region_pack");
    pub(super) const COMPRESS_REGION_PACK: Self = Self::new("compress_region_pack");
    pub(super) const DELETE_CHUNKS: Self = Self::new("delete_chunks");
    pub(super) const WRITE_STRUCTURE: Self = Self::new("write_structure");
    pub(super) const READ_STRUCTURE: Self = Self::new("read_structure");
    pub(super) const ENCODE_IMAGE: Self = Self::new("encode_image");
    pub(super) const PASTE_STRUCTURE: Self = Self::new("paste_structure");
    pub(super) const BUILD_PASTE_PREVIEW: Self = Self::new("build_paste_preview");
    pub(super) const GENERATE_IMAGE: Self = Self::new("generate_image");
    pub(super) const BUILD_OBJ_MODEL: Self = Self::new("build_obj_model");
    pub(super) const STAGE_HISTORY_RECORD: Self = Self::new("stage_history_record");
    pub(super) const COMMIT_CHUNK_WRITE: Self = Self::new("commit_chunk_write");
    pub(super) const EXPORT_STRUCTURE: Self = Self::new("export_structure");
    pub(super) const EXPORT_REGION_PACK: Self = Self::new("export_region_pack");
    pub(super) const EXPORT_OBJ: Self = Self::new("export_obj");
    pub(super) const IMPORT_STRUCTURE: Self = Self::new("import_structure");
    pub(super) const IMPORT_REGION_PACK: Self = Self::new("import_region_pack");
    pub(super) const PREPARE_PASTE_CHUNKS: Self = Self::new("prepare_paste_chunks");
    pub(super) const PREPARE_TRANSFORM_CHUNKS: Self = Self::new("prepare_transform_chunks");
    pub(super) const WRITE_FILE: Self = Self::new("write_file");
    pub(super) const WRITE_OBJ_FILE: Self = Self::new("write_obj_file");
    pub(super) const DELETE_BLOCK_ENTITIES: Self = Self::new("delete_block_entities");
    pub(super) const DELETE_ACTORS: Self = Self::new("delete_actors");
    pub(super) const RESET_CHUNK: Self = Self::new("reset_chunk");
    pub(super) const WRITE_TRANSFORMED_CHUNKS: Self = Self::new("write_transformed_chunks");
    pub(super) const PREPARE_STRUCTURE: Self = Self::new("prepare_structure");
    pub(super) const UNDO_HISTORY: Self = Self::new("undo_history");
    pub(super) const REDO_HISTORY: Self = Self::new("redo_history");
    pub(super) const RESTORE_HISTORY: Self = Self::new("restore_history");
}
