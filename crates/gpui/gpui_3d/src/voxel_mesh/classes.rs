//! Compact neighbor masks without narrowing the public 32-bit class/mask contract.
#[derive(Debug, PartialEq)]
pub(crate) enum Classes {
    Bytes(Box<[u8]>),
    Words(Box<[u32]>),
}
impl Classes {
    pub(super) fn new(values: Vec<u32>) -> Self {
        if values.iter().all(|&v| v <= u8::MAX as u32) {
            Self::Bytes(values.into_iter().map(|v| v as u8).collect())
        } else {
            Self::Words(values.into_boxed_slice())
        }
    }
    pub(crate) fn is_packed(&self) -> bool {
        matches!(self, Self::Bytes(_))
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        match self {
            Self::Bytes(v) => v.len(),
            Self::Words(v) => v.len() * 4,
        }
    }
    pub(super) fn cell(&self, index: usize) -> u32 {
        match self {
            Self::Bytes(v) => u32::from(v[index]),
            Self::Words(v) => v[index],
        }
    }
    pub(crate) fn encode(&self, output: &mut Vec<u32>) {
        match self {
            Self::Words(values) => output.extend(values),
            Self::Bytes(values) => output.extend(values.chunks(4).map(|chunk| {
                chunk.iter().enumerate().fold(0u32, |word, (lane, &value)| {
                    word | (u32::from(value) << (lane * 8))
                })
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_classes_preserve_byte_order_and_wide_masks() {
        let packed = Classes::new(vec![1, 2, 4, 8, 255]);
        let mut words = Vec::new();
        packed.encode(&mut words);
        assert_eq!(words, [0x08040201, 255]);
        assert_eq!(packed.storage_bytes(), 5);
        assert_eq!(packed.cell(4), 255);
        let wide = Classes::new(vec![0, 0x10000]);
        let mut words = Vec::new();
        wide.encode(&mut words);
        assert!(!wide.is_packed());
        assert_eq!(words, [0, 0x10000]);
        assert_eq!(wide.cell(1), 0x10000);
    }
}
