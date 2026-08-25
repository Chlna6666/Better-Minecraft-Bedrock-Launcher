#![expect(
    unsafe_code,
    reason = "WebP rendering crosses the audited libwebp FFI boundary"
)]

use crate::{ObjectFit, Result, size};
use smallvec::SmallVec;
use std::mem::MaybeUninit;

use super::resample::{
    bgra_byte_len, intermediate_sample_size, resize_rgba_frame, rgba_image_from_bgra,
};
use crate::assets::{
    AnimatedFrame, RenderImage, acquire_bitmap_buffer_capacity, release_bitmap_buffer,
};
use crate::assets::{ImageRenderInfo, ImageRenderSize};

pub(super) fn render_sized(
    bytes: &[u8],
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<Option<(RenderImage, ImageRenderInfo)>> {
    let Some((output, decoded_target, original_width, original_height, initial_render_path)) =
        bgra_pixels(bytes, Some((target, object_fit)))?
    else {
        return Ok(None);
    };
    let original_size = size(original_width, original_height);
    let fitted_target = target.fit(original_size, object_fit);
    let (image, render_path) = if decoded_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, decoded_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            initial_render_path,
        )
    } else {
        let rgba = rgba_image_from_bgra(output, decoded_target)?;
        let (rgba, render_path) = resize_rgba_frame(rgba, fitted_target, initial_render_path)?;
        let frame = AnimatedFrame::from_rgba_image(0, rgba);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            render_path,
        )
    };
    Ok(Some((
        image,
        ImageRenderInfo {
            original_width,
            original_height,
            size: fitted_target,
            render_path,
        },
    )))
}

pub(super) fn frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let (output, target, _, _, _) = bgra_pixels(bytes, None)?
        .ok_or_else(|| anyhow::anyhow!("animated WebP requires the animation decoder"))?;
    Ok(AnimatedFrame::from_bgra_bytes(0, target.size(), output))
}

fn bgra_pixels(
    bytes: &[u8],
    target: Option<(ImageRenderSize, ObjectFit)>,
) -> Result<Option<(Vec<u8>, ImageRenderSize, u32, u32, &'static str)>> {
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
    if config.input.has_animation != 0 {
        return Ok(None);
    }

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
        let fitted_target = target.fit(source_size, object_fit);
        intermediate_sample_size(source_size, fitted_target)
    } else {
        ImageRenderSize {
            width: original_width,
            height: original_height,
        }
    };
    let output_len = bgra_byte_len(fitted_target)?;
    let mut output = acquire_bitmap_buffer_capacity(output_len);
    anyhow::ensure!(
        output.capacity() >= output_len,
        "bitmap pool returned insufficient WebP output capacity"
    );

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
                size: output_len,
            },
        },
        pad: [0; 4],
        private_memory: std::ptr::null_mut(),
    };

    let status = unsafe {
        // SAFETY: config.output points at `output`, which is sized for scaled BGRA pixels and remains live.
        WebPDecode(bytes.as_ptr(), bytes.len(), &mut config)
    };
    if status != VP8_STATUS_OK {
        release_bitmap_buffer(output);
        anyhow::bail!("libwebp decode failed: status {status}");
    }
    unsafe {
        // SAFETY: a successful WebPDecode with an external BGRA buffer initializes exactly
        // width * height * 4 bytes, which is `output_len` by construction above.
        output.set_len(output_len);
    }

    Ok(Some((
        output,
        fitted_target,
        original_width,
        original_height,
        if config.options.use_scaling != 0 {
            "webp_scaled"
        } else {
            "webp_direct"
        },
    )))
}
