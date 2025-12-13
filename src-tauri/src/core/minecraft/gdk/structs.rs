use uuid::Uuid;

// --- UserData 相关结构体 ---

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserDataType {
    PackageFiles = 0,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UserDataHeader {
    pub length: u32,
    pub version: u32,
    pub data_type: UserDataType,
    pub unknown: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UserDataPackageFilesHeader {
    pub version: u32,
    pub package_full_name: [u16; 260],
    pub file_count: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UserDataPackageFileEntry {
    pub file_path: [u16; 260],
    pub size: u32,
    pub offset: u32,
}

// --- Segment Metadata ---

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SegmentMetadataHeader {
    pub magic: u32,
    pub version0: u32,
    pub version1: u32,
    pub header_length: u32,
    pub segment_count: u32,
    pub file_paths_length: u32,
    pub pd_uid: [u8; 0x10],
    pub unknown: [u8; 0x3c],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SegmentsAbout {
    pub flags: u16,
    pub path_length: u16,
    pub path_offset: u32,
    pub file_size: u64,
}

// --- XVC Info ---

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XvcEncryptionKeyId {
    pub key_id: [u8; 16],
}

impl XvcEncryptionKeyId {
    // [关键修复] GDK 使用 Microsoft Mixed-Endian 格式存储 GUID
    // 必须使用 from_bytes_le 读取，否则 ID 会匹配失败导致解密错误
    pub fn as_uuid(&self) -> Uuid {
        Uuid::from_bytes_le(self.key_id)
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XvcInfo {
    pub content_id: [u8; 0x10],
    // 对应 C# [MarshalAs(UnmanagedType.ByValArray, SizeConst = 0xC0)]
    // 0xC0 (192) * 16 bytes
    pub encryption_key_ids: [XvcEncryptionKeyId; 192],
    pub description: [u8; 0x100],

    pub version: u32,
    pub region_count: u32,
    pub flags: u32,
    pub padding_d1c: u16,
    pub encryption_key_count: u16,
    pub unknown_d20: u32,
    pub initial_play_region_id: u32,
    pub initial_play_offset: u64,
    pub file_time_created: i64,
    pub preview_region_id: u32,
    pub update_segment_count: u32,
    pub preview_offset: u64,
    pub unused_space: u64,
    pub region_specifier_count: u32,
    pub reserved_d54: [u8; 0x54],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XvcRegionHeader {
    pub id: u32,
    pub key_id: u16,
    pub padding6: u16,
    pub flags: u32,
    pub first_segment_index: u32,
    pub description: [u8; 0x40], // 64 bytes
    pub offset: u64,
    pub length: u64,
    pub hash: u64,
    pub unknown68: u64,
    pub unknown70: u64,
    pub unknown78: u64,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XvcUpdateSegment {
    pub page_num: u32,
    pub hash: u64,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XvcRegionSpecifier {
    pub region_id: u32,
    pub padding4: u32,
    pub key: [u16; 0x40],
    pub value: [u16; 0x80],
}