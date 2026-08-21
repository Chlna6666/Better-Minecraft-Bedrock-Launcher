use crate::error::{LevelDbError, Result};
use crate::options::CompressionPolicy;

pub(crate) const COMPRESSION_NONE: u8 = 0;
pub(crate) const COMPRESSION_SNAPPY: u8 = 1;
pub(crate) const COMPRESSION_ZLIB: u8 = 2;
pub(crate) const COMPRESSION_BEDROCK_ZLIB: u8 = 4;

#[cfg(feature = "zlib")]
mod zlib {
    use super::*;
    use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
    use std::cell::RefCell;

    const MIN_OUTPUT_CAPACITY: usize = 4 * 1024;
    const GROWTH_FLOOR: usize = 4 * 1024;

    pub(super) struct CodecScratch {
        zlib_compressor: Compress,
        raw_compressor: Compress,
        zlib_decompressor: Decompress,
        raw_decompressor: Decompress,
        compressed: Vec<u8>,
        decompressed: Vec<u8>,
    }

    impl CodecScratch {
        fn new() -> Self {
            Self {
                zlib_compressor: Compress::new(Compression::fast(), true),
                raw_compressor: Compress::new(Compression::fast(), false),
                zlib_decompressor: Decompress::new(true),
                raw_decompressor: Decompress::new(false),
                compressed: Vec::with_capacity(MIN_OUTPUT_CAPACITY),
                decompressed: Vec::with_capacity(MIN_OUTPUT_CAPACITY),
            }
        }
    }

    thread_local! {
        static CODEC_SCRATCH: RefCell<CodecScratch> = RefCell::new(CodecScratch::new());
    }

    pub(super) fn with_compressed<T>(
        payload: &[u8],
        zlib_header: bool,
        consume: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        CODEC_SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            compress_reused(&mut scratch, payload, zlib_header)?;
            consume(&scratch.compressed)
        })
    }

    pub(super) fn with_decompressed<T>(
        payload: &[u8],
        zlib_header: bool,
        consume: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        CODEC_SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            decompress_reused(&mut scratch, payload, zlib_header)?;
            consume(&scratch.decompressed)
        })
    }

    fn compress_reused(
        scratch: &mut CodecScratch,
        payload: &[u8],
        zlib_header: bool,
    ) -> Result<()> {
        let initial_capacity = payload
            .len()
            .saturating_add(payload.len() / 8)
            .saturating_add(256)
            .max(MIN_OUTPUT_CAPACITY);
        let CodecScratch {
            zlib_compressor,
            raw_compressor,
            compressed,
            ..
        } = scratch;
        compressed.clear();
        ensure_capacity(compressed, initial_capacity);
        let compressor = if zlib_header {
            zlib_compressor
        } else {
            raw_compressor
        };
        compressor.reset();
        let mut input_offset = 0_usize;

        loop {
            let before_in = compressor.total_in();
            let before_out = compressor.total_out();
            let status = compressor
                .compress_vec(
                    &payload[input_offset..],
                    compressed,
                    FlushCompress::Finish,
                )
                .map_err(|error| LevelDbError::compression("table", error.to_string()))?;
            let consumed = usize::try_from(compressor.total_in().saturating_sub(before_in))
                .unwrap_or(usize::MAX)
                .min(payload.len().saturating_sub(input_offset));
            let produced = usize::try_from(compressor.total_out().saturating_sub(before_out))
                .unwrap_or(usize::MAX);
            input_offset = input_offset.saturating_add(consumed);

            if status == Status::StreamEnd {
                return Ok(());
            }
            if consumed == 0 && produced == 0 {
                if input_offset == payload.len() && compressed.len() < compressed.capacity() {
                    return Err(LevelDbError::compression(
                        "table",
                        "compressor did not reach stream end".to_string(),
                    ));
                }
                grow(compressed);
            } else if compressed.len() == compressed.capacity() {
                grow(compressed);
            }
        }
    }

    fn decompress_reused(
        scratch: &mut CodecScratch,
        payload: &[u8],
        zlib_header: bool,
    ) -> Result<()> {
        let initial_capacity = payload
            .len()
            .saturating_mul(3)
            .max(MIN_OUTPUT_CAPACITY);
        let CodecScratch {
            zlib_decompressor,
            raw_decompressor,
            decompressed,
            ..
        } = scratch;
        decompressed.clear();
        ensure_capacity(decompressed, initial_capacity);
        let decompressor = if zlib_header {
            zlib_decompressor
        } else {
            raw_decompressor
        };
        decompressor.reset(zlib_header);
        let mut input_offset = 0_usize;

        loop {
            let before_in = decompressor.total_in();
            let before_out = decompressor.total_out();
            let status = decompressor
                .decompress_vec(
                    &payload[input_offset..],
                    decompressed,
                    FlushDecompress::Finish,
                )
                .map_err(|error| LevelDbError::compression("table", error.to_string()))?;
            let consumed = usize::try_from(decompressor.total_in().saturating_sub(before_in))
                .unwrap_or(usize::MAX)
                .min(payload.len().saturating_sub(input_offset));
            let produced = usize::try_from(decompressor.total_out().saturating_sub(before_out))
                .unwrap_or(usize::MAX);
            input_offset = input_offset.saturating_add(consumed);

            if status == Status::StreamEnd {
                return Ok(());
            }
            if consumed == 0 && produced == 0 {
                if input_offset == payload.len() && decompressed.len() < decompressed.capacity() {
                    return Err(LevelDbError::compression(
                        "table",
                        "compressed stream ended before stream terminator".to_string(),
                    ));
                }
                grow(decompressed);
            } else if decompressed.len() == decompressed.capacity() {
                grow(decompressed);
            }
        }
    }

    fn ensure_capacity(buffer: &mut Vec<u8>, required: usize) {
        if buffer.capacity() < required {
            buffer.reserve(required.saturating_sub(buffer.len()));
        }
    }

    fn grow(buffer: &mut Vec<u8>) {
        let additional = buffer.capacity().max(GROWTH_FLOOR);
        buffer.reserve(additional);
    }
}

#[must_use]
pub(crate) const fn compression_tag(policy: CompressionPolicy) -> u8 {
    match policy {
        CompressionPolicy::None => COMPRESSION_NONE,
        CompressionPolicy::Snappy => COMPRESSION_SNAPPY,
        CompressionPolicy::Zlib => COMPRESSION_ZLIB,
        CompressionPolicy::RawDeflate => COMPRESSION_BEDROCK_ZLIB,
    }
}

pub(crate) fn with_compressed<T>(
    policy: CompressionPolicy,
    payload: &[u8],
    consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    match policy {
        CompressionPolicy::None => consume(payload),
        CompressionPolicy::Snappy => {
            let encoded = compress_snappy(payload)?;
            consume(&encoded)
        }
        CompressionPolicy::Zlib => with_zlib_compressed(payload, true, consume),
        CompressionPolicy::RawDeflate => with_zlib_compressed(payload, false, consume),
    }
}

pub(crate) fn with_decompressed<T>(
    tag: u8,
    payload: &[u8],
    consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    match tag {
        COMPRESSION_NONE => consume(payload),
        COMPRESSION_SNAPPY => {
            let decoded = decompress_snappy(payload)?;
            consume(&decoded)
        }
        COMPRESSION_ZLIB => with_zlib_decompressed(payload, true, consume),
        COMPRESSION_BEDROCK_ZLIB => with_zlib_decompressed(payload, false, consume),
        other => Err(LevelDbError::compression(
            "table",
            format!("unknown table compression tag {other}"),
        )),
    }
}

pub(crate) fn decompress_owned(tag: u8, payload: &[u8]) -> Result<Vec<u8>> {
    with_decompressed(tag, payload, |decoded| Ok(decoded.to_vec()))
}

#[cfg(feature = "zlib")]
fn with_zlib_compressed<T>(
    payload: &[u8],
    zlib_header: bool,
    consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    zlib::with_compressed(payload, zlib_header, consume)
}

#[cfg(not(feature = "zlib"))]
fn with_zlib_compressed<T>(
    _payload: &[u8],
    _zlib_header: bool,
    _consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    Err(LevelDbError::unsupported(
        "zlib",
        "zlib feature is disabled",
    ))
}

#[cfg(feature = "zlib")]
fn with_zlib_decompressed<T>(
    payload: &[u8],
    zlib_header: bool,
    consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    zlib::with_decompressed(payload, zlib_header, consume)
}

#[cfg(not(feature = "zlib"))]
fn with_zlib_decompressed<T>(
    _payload: &[u8],
    _zlib_header: bool,
    _consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    Err(LevelDbError::unsupported(
        "zlib",
        "zlib feature is disabled",
    ))
}

#[cfg(feature = "snappy")]
fn compress_snappy(payload: &[u8]) -> Result<Vec<u8>> {
    snap::raw::Encoder::new()
        .compress_vec(payload)
        .map_err(|error| LevelDbError::compression("table", error.to_string()))
}

#[cfg(not(feature = "snappy"))]
fn compress_snappy(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "snappy",
        "snappy feature is disabled",
    ))
}

#[cfg(feature = "snappy")]
fn decompress_snappy(payload: &[u8]) -> Result<Vec<u8>> {
    snap::raw::Decoder::new()
        .decompress_vec(payload)
        .map_err(|error| LevelDbError::compression("table", error.to_string()))
}

#[cfg(not(feature = "snappy"))]
fn decompress_snappy(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "snappy",
        "snappy feature is disabled",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "zlib")]
    #[test]
    fn reused_raw_deflate_state_roundtrips_repeated_blocks() {
        for index in 0..64_u8 {
            let payload = vec![index; 4096];
            let encoded = with_compressed(CompressionPolicy::RawDeflate, &payload, |encoded| {
                Ok(encoded.to_vec())
            })
            .expect("compress");
            let decoded = decompress_owned(COMPRESSION_BEDROCK_ZLIB, &encoded).expect("decompress");
            assert_eq!(decoded, payload);
        }
    }

    #[cfg(feature = "zlib")]
    #[test]
    fn reused_zlib_state_roundtrips_repeated_blocks() {
        for index in 0..32_u8 {
            let payload = vec![index; 8192];
            let encoded = with_compressed(CompressionPolicy::Zlib, &payload, |encoded| {
                Ok(encoded.to_vec())
            })
            .expect("compress");
            let decoded = decompress_owned(COMPRESSION_ZLIB, &encoded).expect("decompress");
            assert_eq!(decoded, payload);
        }
    }
}
