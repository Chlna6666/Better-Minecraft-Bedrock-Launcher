//! In-memory access to persisted `level.dat` fields without inventing defaults.

use super::LevelDatDocument;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

impl LevelDatDocument {
    /// Returns whether a root field exists, including fields with a zero or false value.
    ///
    /// # Errors
    /// Returns an error if the document root is not a compound.
    pub fn has_field(&self, name: &str) -> Result<bool> {
        Ok(self.fields()?.contains_key(name))
    }

    /// Borrows a persisted root field. Missing fields remain `None`.
    ///
    /// # Errors
    /// Returns an error if the document root is not a compound.
    pub fn field(&self, name: &str) -> Result<Option<&NbtTag>> {
        Ok(self.fields()?.get(name))
    }

    /// Borrows a persisted field for editing without creating it.
    ///
    /// # Errors
    /// Returns an error if the document root is not a compound.
    pub fn field_mut(&mut self, name: &str) -> Result<Option<&mut NbtTag>> {
        Ok(self.fields_mut()?.get_mut(name))
    }

    /// Inserts or replaces a root field and returns its previous value.
    ///
    /// The caller supplies the exact persisted NBT type. Other fields are untouched;
    /// this operation performs no version conversion or file I/O.
    ///
    /// # Errors
    /// Returns an error if the root is not a compound or `value` is an NBT end marker.
    pub fn set_field(&mut self, name: String, value: NbtTag) -> Result<Option<NbtTag>> {
        if matches!(value, NbtTag::End) {
            return Err(BedrockWorldError::Validation(
                "level.dat fields cannot be NBT end markers".to_string(),
            ));
        }
        Ok(self.fields_mut()?.insert(name, value))
    }

    /// Removes a field without changing the relative order of surviving fields.
    ///
    /// # Errors
    /// Returns an error if the document root is not a compound.
    pub fn remove_field(&mut self, name: &str) -> Result<Option<NbtTag>> {
        Ok(self.fields_mut()?.shift_remove(name))
    }

    fn fields(&self) -> Result<&IndexMap<String, NbtTag>> {
        match &self.root {
            NbtTag::Compound(fields) => Ok(fields),
            _ => Err(invalid_root()),
        }
    }

    fn fields_mut(&mut self) -> Result<&mut IndexMap<String, NbtTag>> {
        match &mut self.root {
            NbtTag::Compound(fields) => Ok(fields),
            _ => Err(invalid_root()),
        }
    }
}

fn invalid_root() -> BedrockWorldError {
    BedrockWorldError::CorruptWorld("level.dat root is not a compound".to_string())
}
