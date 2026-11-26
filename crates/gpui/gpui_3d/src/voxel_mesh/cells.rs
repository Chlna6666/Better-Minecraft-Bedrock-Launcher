//! Compact retained model indices while preserving the non-emitting halo sentinel.
use super::VOXEL_OCCLUDER;

#[derive(Debug, PartialEq)]
pub(crate) enum Cells {
    Bytes(Box<[u8]>),
    Words(Box<[u32]>),
}

impl Cells {
    pub(super) fn new(values: Vec<u32>) -> Self {
        if values
            .iter()
            .all(|&v| v < u8::MAX as u32 || v == VOXEL_OCCLUDER)
        {
            Self::Bytes(values.into_iter().map(|v| v as u8).collect())
        } else {
            Self::Words(values.into_boxed_slice())
        }
    }
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Bytes(values) => values.len(),
            Self::Words(values) => values.len(),
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        match self {
            Self::Bytes(values) => values.len(),
            Self::Words(values) => values.len() * 4,
        }
    }
    pub(super) fn cell(&self, index: usize) -> u32 {
        match self {
            Self::Bytes(values) => match values[index] {
                u8::MAX => VOXEL_OCCLUDER,
                value => u32::from(value),
            },
            Self::Words(values) => values[index],
        }
    }
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = u32> + '_ {
        (0..self.len()).map(|index| self.cell(index))
    }
    pub(crate) fn to_vec(&self) -> Vec<u32> {
        self.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_indices_preserve_halo_and_large_model_ids() {
        let values = vec![0, 1, 254, VOXEL_OCCLUDER];
        let cells = Cells::new(values.clone());
        assert_eq!(cells.storage_bytes(), 4);
        assert_eq!(cells.to_vec(), values);
        for value in [255, 256, 65536] {
            let values = vec![0, value, VOXEL_OCCLUDER];
            let cells = Cells::new(values.clone());
            assert_eq!(cells.storage_bytes(), 12);
            assert_eq!(cells.to_vec(), values);
        }
    }
}
