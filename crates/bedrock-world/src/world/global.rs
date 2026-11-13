//! Storage-backed access to global world records.

use super::*;

impl<S: StorageBackend> World<S> {
    /// Reads a single typed global record by exact key.
    ///
    /// # Errors
    ///
    /// Returns storage errors or global NBT parse errors.
    pub fn global(&self, kind: GlobalRecordKind) -> Result<Option<Record>> {
        let key = kind.storage_key();
        self.storage()
            .get(&key)?
            .map(|value| crate::global::parse(kind.clone(), value))
            .transpose()
    }

    /// Scans known global records while preserving each typed key kind.
    ///
    /// # Errors
    ///
    /// Returns storage errors, cancellation, or global NBT parse errors.
    pub fn globals(&self, options: WorldScanOptions) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        self.storage()
            .for_each_entry(to_storage_read_options(&options), &mut |key, value| {
                check_cancelled(&options)?;
                let kind = match BedrockDbKey::decode(key) {
                    BedrockDbKey::Global(kind) => kind,
                    BedrockDbKey::Portals => GlobalRecordKind::Portals,
                    BedrockDbKey::SchedulerWt => GlobalRecordKind::SchedulerWt,
                    _ => return Ok(StorageVisitorControl::Continue),
                };
                records.push(crate::global::parse(kind.clone(), value.clone())?);
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(records)
    }

    /// Reads named Bedrock ticking-area NBT records.
    ///
    /// # Errors
    ///
    /// Returns storage, cancellation, or NBT decoding errors.
    pub fn ticking_areas(&self, options: WorldScanOptions) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        self.storage().for_each_prefix_ref(
            b"tickingarea",
            to_storage_read_options(&options),
            &mut |entry| {
                check_cancelled(&options)?;
                let BedrockDbKey::TickingArea(suffix) = BedrockDbKey::decode(entry.key) else {
                    return Ok(StorageVisitorControl::Continue);
                };
                let kind = GlobalRecordKind::TickingArea(suffix);
                records.push(crate::global::parse(
                    kind.clone(),
                    Bytes::copy_from_slice(entry.value),
                )?);
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(records)
    }

    /// Reads named Bedrock structure-template NBT records.
    ///
    /// # Errors
    ///
    /// Returns storage, cancellation, or NBT decoding errors.
    pub fn structure_templates(&self, options: WorldScanOptions) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        self.storage().for_each_prefix_ref(
            b"structuretemplate",
            to_storage_read_options(&options),
            &mut |entry| {
                check_cancelled(&options)?;
                let BedrockDbKey::StructureTemplate(suffix) = BedrockDbKey::decode(entry.key)
                else {
                    return Ok(StorageVisitorControl::Continue);
                };
                let kind = GlobalRecordKind::StructureTemplate(suffix);
                records.push(crate::global::parse(
                    kind.clone(),
                    Bytes::copy_from_slice(entry.value),
                )?);
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(records)
    }

    /// Writes a global record after serialize -> parse roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation
    /// errors for malformed records, or storage errors from the commit.
    pub fn save_global(&self, record: &Record) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.save_global(record)?;
        transaction.commit()
    }

    /// Deletes a typed global record.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds or storage
    /// errors from the commit.
    pub fn delete_global(&self, kind: GlobalRecordKind) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.delete_raw_key(kind.storage_key());
        transaction.commit()
    }

    /// Returns whether the exact global key exists, without parsing its payload.
    ///
    /// # Errors
    /// Returns storage read errors.
    pub fn has_global(&self, kind: &GlobalRecordKind) -> Result<bool> {
        Ok(self.storage().get(&kind.storage_key())?.is_some())
    }

    /// Removes a global record only if its read snapshot is still current.
    ///
    /// # Errors
    /// Returns read-only, stale-source or storage errors.
    pub fn remove_global(&self, record: &Record) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.remove_global(record);
        transaction.commit()
    }
}
