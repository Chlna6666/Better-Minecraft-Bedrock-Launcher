use crate::{ObjectFit, Result, Size, size};
use smallvec::SmallVec;
use std::mem::MaybeUninit;

use super::target::{
    bgra_bytes_to_rgba_image, bgra_len, fitted_target_size, high_quality_intermediate_target,
    resize_rgba_to_target,
};
use crate::assets::render_image::{AnimatedFrame, RenderImage};
use crate::assets::types::{ImageDecodeTarget, TargetImageDecodeMetadata};

pub(super) fn decode_webp_to_target(
    bytes: &[u8],
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let original_size = decode_webp_dimensions(bytes)?;
    let fitted_target = fitted_target_size(original_size, target, object_fit);
    let sample_target = high_quality_intermediate_target(original_size, fitted_target);
    let (output, decoded_target, original_width, original_height, initial_decode_mode) =
        decode_webp_bgra(bytes, Some((sample_target, object_fit)))?;
    let (image, decode_mode) = if decoded_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, decoded_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            initial_decode_mode,
        )
    } else {
        let rgba = bgra_bytes_to_rgba_image(output, decoded_target)?;
        let (rgba, decode_mode) = resize_rgba_to_target(rgba, fitted_target, initial_decode_mode)?;
        let frame = AnimatedFrame::from_rgba_image(0, rgba);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            decode_mode,
        )
    };
    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width,
            original_height,
            target: fitted_target,
            decode_mode,
        },
    ))
}

pub(super) fn decode_static_webp_frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let (output, target, _, _, _) = decode_webp_bgra(bytes, None)?;
    Ok(AnimatedFrame::from_bgra_bytes(0, target.size(), output))
}

fn decode_webp_bgra(
    bytes: &[u8],
    target: Option<(ImageDecodeTarget, ObjectFit)>,
) -> Result<(Vec<u8>, ImageDecodeTarget, u32, u32, &'static str)> {
    use libwebp_sys::{
        MODE_BGRA, VP8_STATUS_OK, WebPDecBuffer, WebPDecode, WebPDecoderConfig, WebPGetFeatures,
        WebPInitDecoderConfig, WebPRGBABuffer,
    };

    let mut config = MaybeUninit::<WebPDecoderConfig>::uninit();
    let init_ok = unsafe {
        // SAFETY: `config` points to valid writable storage for libwebp initialization.
        WebPInitDecoderConfig(config.as_mut_ptr())
    };
    anyhow::ensure!(
        init_ok != 0,
        "libwebp decoder configuration initialization failed"
    );
    let mut config = unsafe {
        // SAFETY: libwebp reported successful initialization above.
        config.assume_init()
    };

    let feature_status = unsafe {
        // SAFETY: `bytes` is a valid byte slice for the duration of this call and config.input is initialized.
        WebPGetFeatures(bytes.as_ptr(), bytes.len(), &mut config.input)
    };
    anyhow::ensure!(
        feature_status == VP8_STATUS_OK,
        "libwebp failed to read features: status {feature_status}"
    );
    anyhow::ensure!(
        config.input.has_animation == 0,
        "target-size animated WebP decode is not supported"
    );

    let original_width = u32::try_from(config.input.width)
        .ok()
        .filter(|width| *width > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source width"))?;
    let original_height = u32::try_from(config.input.height)
        .ok()
        .filter(|height| *height > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source height"))?;
    let source_size = size(original_width, original_height);
    let fitted_target = if let Some((target, object_fit)) = target {
        fitted_target_size(source_size, target, object_fit)
    } else {
        ImageDecodeTarget {
            width: original_width,
            height: original_height,
        }
    };
    let output_len = bgra_len(fitted_target)?;
    let mut output = vec![0; output_len];

    config.options.use_scaling =
        i32::from(fitted_target.width != original_width || fitted_target.height != original_height);
    config.options.scaled_width = fitted_target.width as i32;
    config.options.scaled_height = fitted_target.height as i32;
    config.output = WebPDecBuffer {
        colorspace: MODE_BGRA,
        width: fitted_target.width as i32,
        height: fitted_target.height as i32,
        is_external_memory: 1,
        u: libwebp_sys::__WebPDecBufferUnion {
            RGBA: WebPRGBABuffer {
                rgba: output.as_mut_ptr(),
                stride: fitted_target.width as i32 * 4,
                size: output.len(),
            },
        },
        pad: [0; 4],
        private_memory: std::ptr::null_mut(),
    };

    let status = unsafe {
        // SAFETY: config.output points at `output`, which is sized for scaled BGRA pixels and remains live.
        WebPDecode(bytes.as_ptr(), bytes.len(), &mut config)
    };
    anyhow::ensure!(
        status == VP8_STATUS_OK,
        "libwebp decode failed: status {status}"
    );

    Ok((
        output,
        fitted_target,
        original_width,
        original_height,
        if config.options.use_scaling != 0 {
            "webp_scaled_decode"
        } else {
            "webp_direct_decode"
        },
    ))
}

fn decode_webp_dimensions(bytes: &[u8]) -> Result<Size<u32>> {
    use libwebp_sys::{VP8_STATUS_OK, WebPBitstreamFeatures, WebPGetFeatures};

    let mut features = MaybeUninit::<WebPBitstreamFeatures>::uninit();
    let status = unsafe {
        // SAFETY: `features` points to writable storage and `bytes` lives for this call.
        WebPGetFeatures(bytes.as_ptr(), bytes.len(), features.as_mut_ptr())
    };
    anyhow::ensure!(
        status == VP8_STATUS_OK,
        "libwebp failed to read features: status {status}"
    );
    let features = unsafe {
        // SAFETY: libwebp reported successful feature parsing above.
        features.assume_init()
    };
    let width = u32::try_from(features.width)
        .ok()
        .filter(|width| *width > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source width"))?;
    let height = u32::try_from(features.height)
        .ok()
        .filter(|height| *height > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source height"))?;
    Ok(size(width, height))
}
