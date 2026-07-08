use crate::music::types::DecodedCoverImage;
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::Path;
use std::time::Instant;

use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;

const COVER_THUMB_MAX_SIZE: u32 = 128;
const ID3_HEADER_LEN: usize = 10;
const ID3_MAX_TAG_BYTES: usize = 32 * 1024 * 1024;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub fn cover_fingerprint(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);

    if let Ok(metadata) = std::fs::metadata(path) {
        metadata.len().hash(&mut hasher);
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            duration.as_nanos().hash(&mut hasher);
        }
    }

    hasher.finish()
}

fn picture_with_type(
    tagged_file: &impl TaggedFileExt,
    picture_type: PictureType,
) -> Option<&Picture> {
    tagged_file
        .tags()
        .iter()
        .filter_map(|tag| tag.get_picture_type(picture_type))
        .find(|picture| !picture.data().is_empty())
}

fn first_cover_picture(tagged_file: &impl TaggedFileExt) -> Option<&Picture> {
    picture_with_type(tagged_file, PictureType::CoverFront).or_else(|| {
        tagged_file
            .tags()
            .iter()
            .flat_map(|tag| tag.pictures())
            .find(|picture| !picture.data().is_empty())
    })
}

pub(super) fn has_embedded_cover(tagged_file: &impl TaggedFileExt) -> bool {
    first_cover_picture(tagged_file).is_some()
}

fn decode_picture_thumbnail(picture: &Picture, started: Instant) -> Option<DecodedCoverImage> {
    let source_byte_len = picture.data().len();
    decode_image_thumbnail(picture.data(), source_byte_len, started)
}

fn decode_image_thumbnail(
    image_data: &[u8],
    source_byte_len: usize,
    started: Instant,
) -> Option<DecodedCoverImage> {
    let image = image::load_from_memory(image_data).ok()?;
    let mut thumbnail = image
        .resize_to_fill(
            COVER_THUMB_MAX_SIZE,
            COVER_THUMB_MAX_SIZE,
            image::imageops::FilterType::Triangle,
        )
        .into_rgba8();

    for pixel in thumbnail.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let (width, height) = thumbnail.dimensions();
    Some(DecodedCoverImage {
        width,
        height,
        bgra_pixels: thumbnail.into_raw(),
        source_byte_len,
        decode_elapsed: started.elapsed(),
    })
}

fn decode_tagged_file_cover_thumbnail(
    tagged_file: &impl TaggedFileExt,
    started: Instant,
) -> Option<DecodedCoverImage> {
    for picture in tagged_file
        .tags()
        .iter()
        .flat_map(|tag| tag.pictures())
        .filter(|picture| picture.pic_type() == PictureType::CoverFront)
    {
        if let Some(decoded) = decode_picture_thumbnail(picture, started) {
            return Some(decoded);
        }
    }

    tagged_file
        .tags()
        .iter()
        .flat_map(|tag| tag.pictures())
        .filter(|picture| picture.pic_type() != PictureType::CoverFront)
        .find_map(|picture| decode_picture_thumbnail(picture, started))
}

pub fn decode_cover_thumbnail(track_path: &Path) -> Option<DecodedCoverImage> {
    let started = Instant::now();
    Probe::open(track_path)
        .ok()
        .and_then(|probe| probe.read().ok())
        .as_ref()
        .and_then(|tagged_file| decode_tagged_file_cover_thumbnail(tagged_file, started))
        .or_else(|| decode_raw_id3v2_apic_thumbnail(track_path, started))
}

fn decode_raw_id3v2_apic_thumbnail(
    track_path: &Path,
    started: Instant,
) -> Option<DecodedCoverImage> {
    let mut file = File::open(track_path).ok()?;
    let mut header = [0; ID3_HEADER_LEN];
    file.read_exact(&mut header).ok()?;
    if &header[..3] != b"ID3" {
        return None;
    }

    let tag_size = read_synchsafe_usize(&header[6..10])?;
    if tag_size > ID3_MAX_TAG_BYTES {
        return None;
    }

    let mut tag_bytes = vec![0; ID3_HEADER_LEN + tag_size];
    tag_bytes[..ID3_HEADER_LEN].copy_from_slice(&header);
    file.read_exact(&mut tag_bytes[ID3_HEADER_LEN..]).ok()?;
    decode_id3v2_apic_thumbnail_from_bytes(&tag_bytes, started)
}

fn decode_id3v2_apic_thumbnail_from_bytes(
    tag_bytes: &[u8],
    started: Instant,
) -> Option<DecodedCoverImage> {
    if tag_bytes.len() < ID3_HEADER_LEN || &tag_bytes[..3] != b"ID3" {
        return None;
    }

    let version = tag_bytes[3];
    if !matches!(version, 2..=4) {
        return None;
    }

    let tag_size = read_synchsafe_usize(&tag_bytes[6..10])?;
    let tag_end = ID3_HEADER_LEN.checked_add(tag_size)?.min(tag_bytes.len());
    let mut offset = ID3_HEADER_LEN;
    let mut first_decoded = None;

    while offset < tag_end {
        let Some(frame) = read_id3v2_frame(tag_bytes, tag_end, offset, version) else {
            break;
        };
        if frame.frame_id.iter().all(|byte| *byte == 0) {
            break;
        }
        offset = frame.next_offset;

        if !frame.is_attached_picture(version) {
            continue;
        }

        let Some((picture_type, image_data)) = apic_image_data(version, frame.content) else {
            continue;
        };
        let Some(decoded) = decode_image_thumbnail(image_data, image_data.len(), started) else {
            continue;
        };
        if picture_type == PictureType::CoverFront {
            return Some(decoded);
        }
        first_decoded.get_or_insert(decoded);
    }

    first_decoded
}

struct Id3v2Frame<'a> {
    frame_id: &'a [u8],
    content: &'a [u8],
    next_offset: usize,
}

impl Id3v2Frame<'_> {
    fn is_attached_picture(&self, version: u8) -> bool {
        if version == 2 {
            self.frame_id == b"PIC"
        } else {
            self.frame_id == b"APIC"
        }
    }
}

fn read_id3v2_frame<'a>(
    tag_bytes: &'a [u8],
    tag_end: usize,
    offset: usize,
    version: u8,
) -> Option<Id3v2Frame<'a>> {
    let (header_len, frame_id, frame_size) = if version == 2 {
        let header_end = offset.checked_add(6)?;
        if header_end > tag_end {
            return None;
        }
        (
            6,
            &tag_bytes[offset..offset + 3],
            read_u24_usize(&tag_bytes[offset + 3..offset + 6])?,
        )
    } else {
        let header_end = offset.checked_add(10)?;
        if header_end > tag_end {
            return None;
        }
        let frame_size = if version == 4 {
            read_synchsafe_usize(&tag_bytes[offset + 4..offset + 8])?
        } else {
            read_u32_usize(&tag_bytes[offset + 4..offset + 8])?
        };
        (10, &tag_bytes[offset..offset + 4], frame_size)
    };

    let content_start = offset.checked_add(header_len)?;
    let content_end = content_start.checked_add(frame_size)?;
    if content_end > tag_end {
        return None;
    }

    Some(Id3v2Frame {
        frame_id,
        content: &tag_bytes[content_start..content_end],
        next_offset: content_end,
    })
}

fn apic_image_data(version: u8, content: &[u8]) -> Option<(PictureType, &[u8])> {
    if content.is_empty() {
        return None;
    }

    let mut offset: usize = 1;
    if version == 2 {
        offset = offset.checked_add(3)?;
    } else {
        let mime_len = content.get(offset..)?.iter().position(|byte| *byte == 0)?;
        offset = offset.checked_add(mime_len + 1)?;
    }

    let picture_type = PictureType::from_u8(*content.get(offset)?);
    offset = offset.checked_add(1)?;
    let image_data = find_image_data(content.get(offset..)?)?;
    Some((picture_type, image_data))
}

fn find_image_data(bytes: &[u8]) -> Option<&[u8]> {
    bytes.iter().enumerate().find_map(|(index, _)| {
        is_supported_image_signature(&bytes[index..]).then_some(&bytes[index..])
    })
}

fn is_supported_image_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff]) || bytes.starts_with(PNG_SIGNATURE)
}

fn read_synchsafe_usize(bytes: &[u8]) -> Option<usize> {
    let bytes: [u8; 4] = bytes.try_into().ok()?;
    if bytes.iter().any(|byte| byte & 0x80 != 0) {
        return None;
    }

    Some(
        ((bytes[0] as usize) << 21)
            | ((bytes[1] as usize) << 14)
            | ((bytes[2] as usize) << 7)
            | bytes[3] as usize,
    )
}

fn read_u32_usize(bytes: &[u8]) -> Option<usize> {
    let bytes: [u8; 4] = bytes.try_into().ok()?;
    Some(u32::from_be_bytes(bytes) as usize)
}

fn read_u24_usize(bytes: &[u8]) -> Option<usize> {
    let bytes: [u8; 3] = bytes.try_into().ok()?;
    Some(((bytes[0] as usize) << 16) | ((bytes[1] as usize) << 8) | bytes[2] as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::file::{FileType, TaggedFile};
    use lofty::properties::FileProperties;
    use lofty::tag::{Tag, TagType};

    fn picture(picture_type: PictureType, data: &[u8]) -> Picture {
        Picture::unchecked(data.to_vec())
            .pic_type(picture_type)
            .build()
    }

    fn tagged_file(tags: Vec<Tag>) -> TaggedFile {
        TaggedFile::new(FileType::Flac, FileProperties::default(), tags)
    }

    fn synchsafe_size(size: usize) -> [u8; 4] {
        [
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]
    }

    #[test]
    fn has_embedded_cover_finds_picture_outside_primary_tag() {
        let mut id3_tag = Tag::new(TagType::Id3v2);
        id3_tag.push_picture(picture(PictureType::CoverFront, &[1, 2, 3]));
        let vorbis_tag = Tag::new(TagType::VorbisComments);
        let tagged_file = tagged_file(vec![id3_tag, vorbis_tag]);

        assert!(has_embedded_cover(&tagged_file));
    }

    #[test]
    fn first_cover_picture_prefers_front_cover() {
        let mut first_tag = Tag::new(TagType::Id3v2);
        first_tag.push_picture(picture(PictureType::CoverBack, &[1]));
        let mut second_tag = Tag::new(TagType::VorbisComments);
        second_tag.push_picture(picture(PictureType::CoverFront, &[2]));
        let tagged_file = tagged_file(vec![first_tag, second_tag]);

        let selected = first_cover_picture(&tagged_file).map(Picture::data);

        assert_eq!(selected, Some(&[2][..]));
    }

    #[test]
    fn has_embedded_cover_ignores_empty_picture_data() {
        let mut tag = Tag::new(TagType::VorbisComments);
        tag.push_picture(picture(PictureType::CoverFront, &[]));
        let tagged_file = tagged_file(vec![tag]);

        assert!(!has_embedded_cover(&tagged_file));
    }

    #[test]
    fn raw_id3v2_apic_decode_recovers_missing_description_terminator() {
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        let mut apic = Vec::new();
        apic.push(0);
        apic.extend_from_slice(b"image/png\0");
        apic.push(PictureType::CoverFront.as_u8());
        apic.extend_from_slice(&png);

        let mut frame = Vec::new();
        frame.extend_from_slice(b"APIC");
        frame.extend_from_slice(&(apic.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&apic);

        let mut tag = Vec::new();
        tag.extend_from_slice(b"ID3\x03\x00\x00");
        tag.extend_from_slice(&synchsafe_size(frame.len()));
        tag.extend_from_slice(&frame);

        let decoded = decode_id3v2_apic_thumbnail_from_bytes(&tag, Instant::now());

        assert_eq!(decoded.map(|image| image.source_byte_len), Some(png.len()));
    }
}
