use super::{
    bmp::decode_static_bmp_frame, jpeg::decode_static_jpeg_frame, png::decode_static_png_frame,
    webp::decode_static_webp_frame,
};
use crate::Result;
use crate::assets::render_image::{AnimatedFrame, AnimatedImageSource, DecodedAnimation};
use image::{
    AnimationDecoder, ImageFormat, Rgba,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use smallvec::SmallVec;
use std::io::Cursor;

pub(super) fn decode_animation_prefix(
    source: &AnimatedImageSource,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            decode_frames_prefix(
                decoder.into_frames(),
                max_resident_frames,
                max_resident_bytes,
            )
        }
        ImageFormat::Png => decode_png_prefix(source, max_resident_frames, max_resident_bytes),
        ImageFormat::WebP => decode_webp_prefix(source, max_resident_frames, max_resident_bytes),
        ImageFormat::Jpeg => Ok(single_frame_decoded(decode_static_jpeg_frame(
            source.bytes.as_ref(),
        )?)),
        ImageFormat::Bmp => Ok(single_frame_decoded(decode_static_bmp_frame(
            source.bytes.as_ref(),
        )?)),
        format => anyhow::bail!("unsupported GPUI image asset format: {format:?}"),
    }
}

fn decode_png_prefix(
    source: &AnimatedImageSource,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.is_apng()? {
        let decoded = decode_frames_prefix(
            decoder.apng()?.into_frames(),
            max_resident_frames,
            max_resident_bytes,
        )?;
        if decoded.first_frame.byte_len() > 0 {
            return Ok(decoded);
        }
    }

    Ok(single_frame_decoded(decode_static_png_frame(
        source.bytes.as_ref(),
    )?))
}

fn decode_webp_prefix(
    source: &AnimatedImageSource,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.has_animation() {
        let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
        return decode_frames_prefix(
            decoder.into_frames(),
            max_resident_frames,
            max_resident_bytes,
        );
    }

    Ok(single_frame_decoded(decode_static_webp_frame(
        source.bytes.as_ref(),
    )?))
}

fn single_frame_decoded(first_frame: AnimatedFrame) -> DecodedAnimation {
    DecodedAnimation {
        first_frame,
        remaining_frames: SmallVec::new(),
        is_complete: true,
    }
}

fn decode_frames_prefix(
    frames: image::Frames<'_>,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    let mut frames = frames.enumerate();
    let Some((_, first_frame)) = frames.next() else {
        return Err(anyhow::anyhow!("animated image did not contain any frames"));
    };
    let first_frame = AnimatedFrame::from_rgba_frame(0, first_frame?);
    let mut decoded_byte_len = first_frame.byte_len();
    let mut remaining_frames = SmallVec::<[AnimatedFrame; 8]>::new();

    for (sequence, frame) in frames {
        if remaining_frames.len() + 1 >= max_resident_frames
            || decoded_byte_len >= max_resident_bytes
        {
            return Ok(DecodedAnimation {
                first_frame,
                remaining_frames,
                is_complete: false,
            });
        }

        let frame = AnimatedFrame::from_rgba_frame(sequence, frame?);
        decoded_byte_len = decoded_byte_len.saturating_add(frame.byte_len());
        if decoded_byte_len > max_resident_bytes {
            return Ok(DecodedAnimation {
                first_frame,
                remaining_frames,
                is_complete: false,
            });
        }
        remaining_frames.push(frame);
    }

    Ok(DecodedAnimation {
        first_frame,
        remaining_frames,
        is_complete: true,
    })
}
