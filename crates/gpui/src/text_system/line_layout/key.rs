use crate::{FontId, Pixels, SharedString};
use smallvec::SmallVec;
use std::{
    borrow::Borrow,
    hash::{Hash, Hasher},
    sync::Arc,
};

/// A run of text with a single font.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FontRun {
    pub(crate) len: usize,
    pub(crate) font_id: FontId,
}

pub(super) trait AsCacheKeyRef {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_>;
}

#[derive(Clone, Debug, Eq)]
pub(super) struct CacheKey {
    pub(super) text: SharedString,
    pub(super) font_size: Pixels,
    pub(super) runs: SmallVec<[FontRun; 1]>,
    pub(super) wrap_width: Option<Pixels>,
    pub(super) force_width: Option<Pixels>,
}

#[derive(Copy, Clone)]
pub(super) struct CacheKeyRef<'a> {
    pub(super) text: &'a str,
    pub(super) font_size: Pixels,
    pub(super) runs: &'a [FontRun],
    pub(super) wrap_width: Option<Pixels>,
    pub(super) force_width: Option<Pixels>,
}

/// Iterates the shaping identity of a font-run slice without allocating.
///
/// Decoration changes are intentionally not part of shaping. Some callers still split otherwise
/// identical font runs at color/background/underline boundaries so that paint can retain those
/// boundaries. Treat adjacent runs using the same resolved font as one canonical run when hashing
/// and comparing layout-cache keys. Zero-length runs are ignored as well.
#[derive(Clone)]
struct CanonicalFontRuns<'a> {
    runs: &'a [FontRun],
    index: usize,
}

impl<'a> CanonicalFontRuns<'a> {
    fn new(runs: &'a [FontRun]) -> Self {
        Self { runs, index: 0 }
    }
}

impl Iterator for CanonicalFontRuns<'_> {
    type Item = FontRun;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.runs.len() && self.runs[self.index].len == 0 {
            self.index += 1;
        }
        let first = *self.runs.get(self.index)?;
        self.index += 1;

        let mut len = first.len;
        while self.index < self.runs.len() {
            let run = self.runs[self.index];
            if run.len == 0 {
                self.index += 1;
                continue;
            }
            if run.font_id != first.font_id {
                break;
            }
            len = len.saturating_add(run.len);
            self.index += 1;
        }

        Some(FontRun {
            len,
            font_id: first.font_id,
        })
    }
}

/// Materializes the unique shaping representation of a run slice.
///
/// This is used only on cache misses before invoking the platform shaper, so the small allocation
/// is off the hot cache-hit path. Keeping the platform input canonical guarantees that a cached
/// layout cannot depend on paint-only decoration boundaries from whichever frame populated it.
pub(super) fn canonicalize_font_runs(runs: &[FontRun]) -> SmallVec<[FontRun; 1]> {
    CanonicalFontRuns::new(runs).collect()
}

impl PartialEq for CacheKeyRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.font_size == other.font_size
            && self.wrap_width == other.wrap_width
            && self.force_width == other.force_width
            && CanonicalFontRuns::new(self.runs).eq(CanonicalFontRuns::new(other.runs))
    }
}

impl Eq for CacheKeyRef<'_> {}

impl Hash for CacheKeyRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
        self.font_size.hash(state);
        self.wrap_width.hash(state);
        self.force_width.hash(state);
        for run in CanonicalFontRuns::new(self.runs) {
            run.hash(state);
        }
    }
}

impl PartialEq for dyn AsCacheKeyRef + '_ {
    fn eq(&self, other: &dyn AsCacheKeyRef) -> bool {
        self.as_cache_key_ref() == other.as_cache_key_ref()
    }
}

impl Eq for dyn AsCacheKeyRef + '_ {}

impl Hash for dyn AsCacheKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state)
    }
}

impl AsCacheKeyRef for CacheKey {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        CacheKeyRef {
            text: &self.text,
            font_size: self.font_size,
            runs: self.runs.as_slice(),
            wrap_width: self.wrap_width,
            force_width: self.force_width,
        }
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_cache_key_ref().eq(&other.as_cache_key_ref())
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state);
    }
}

impl<'a> Borrow<dyn AsCacheKeyRef + 'a> for Arc<CacheKey> {
    fn borrow(&self) -> &(dyn AsCacheKeyRef + 'a) {
        self.as_ref() as &dyn AsCacheKeyRef
    }
}

impl AsCacheKeyRef for CacheKeyRef<'_> {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::px;
    use std::collections::hash_map::DefaultHasher;

    fn hash_key(key: CacheKeyRef<'_>) -> u64 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn decoration_boundaries_do_not_change_layout_cache_identity() {
        let split = [
            FontRun {
                len: 2,
                font_id: FontId(7),
            },
            FontRun {
                len: 3,
                font_id: FontId(7),
            },
        ];
        let merged = [FontRun {
            len: 5,
            font_id: FontId(7),
        }];
        let split_key = CacheKeyRef {
            text: "hello",
            font_size: px(14.0),
            runs: &split,
            wrap_width: None,
            force_width: None,
        };
        let merged_key = CacheKeyRef {
            runs: &merged,
            ..split_key
        };

        assert!(split_key == merged_key);
        assert_eq!(hash_key(split_key), hash_key(merged_key));
        assert_eq!(canonicalize_font_runs(&split).as_slice(), &merged);
    }

    #[test]
    fn real_font_boundaries_remain_part_of_layout_cache_identity() {
        let first = [
            FontRun {
                len: 2,
                font_id: FontId(7),
            },
            FontRun {
                len: 3,
                font_id: FontId(8),
            },
        ];
        let second = [FontRun {
            len: 5,
            font_id: FontId(7),
        }];
        let first_key = CacheKeyRef {
            text: "hello",
            font_size: px(14.0),
            runs: &first,
            wrap_width: None,
            force_width: None,
        };
        let second_key = CacheKeyRef {
            runs: &second,
            ..first_key
        };

        assert!(first_key != second_key);
    }

    #[test]
    fn zero_length_runs_do_not_fragment_layout_cache_identity() {
        let split = [
            FontRun {
                len: 2,
                font_id: FontId(7),
            },
            FontRun {
                len: 0,
                font_id: FontId(8),
            },
            FontRun {
                len: 3,
                font_id: FontId(7),
            },
        ];
        let merged = [FontRun {
            len: 5,
            font_id: FontId(7),
        }];
        let split_key = CacheKeyRef {
            text: "hello",
            font_size: px(14.0),
            runs: &split,
            wrap_width: Some(px(100.0)),
            force_width: None,
        };
        let merged_key = CacheKeyRef {
            runs: &merged,
            ..split_key
        };

        assert!(split_key == merged_key);
        assert_eq!(hash_key(split_key), hash_key(merged_key));
        assert_eq!(canonicalize_font_runs(&split).as_slice(), &merged);
    }
}
