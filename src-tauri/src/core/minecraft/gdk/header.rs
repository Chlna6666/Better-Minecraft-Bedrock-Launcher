//! 此文件包含从 C# GdkDecode 库转换而来的所有数据结构。
// 修复 v13: 修正 MsiXVDHeader 结构体，将 magic 和 sandbox_id 字段的类型从 [u8; N] 改为 [i8; N]，以正确匹配 C# 中的 char[] 类型。

use uuid::Uuid;

#[repr(C, packed)]
#[derive(Debug, Clone)]
#[derive(Copy)]
pub struct MsiXVDHeader {
    pub signature: [u8; 0x200],
    pub magic: [i8; 8],
    pub volumes: MsiXVDVolumeAttributes,
    pub format_version: u32,
    pub file_time_created: i64,
    pub drive_size: u64,
    pub vd_uid: [u8; 0x10],
    pub ud_uid: [u8; 0x10],
    pub top_hash_block_hash: [u8; 0x20],
    pub original_xvc_data_hash: [u8; 0x20],
    pub kind: MsiXVDKind,
    pub category: MsiXVDContentCategory,
    pub embedded_xvd_length: u32,
    pub user_data_length: u32,
    pub xvc_data_length: u32,
    pub dynamic_header_length: u32,
    pub block_size: u32,
    pub ext_entries: [ExtEntry; 4],
    pub capabilities: [u16; 8],
    pub pe_catalog_hash: [u8; 0x20],
    pub embedded_xvd_pd_uid: [u8; 0x10],
    pub reserved13c: [u8; 0x10],
    pub key_material: [u8; 0x20],
    pub user_data_hash: [u8; 0x20],
    pub sandbox_id: [i8; 0x10],
    pub product_id: [u8; 0x10],
    pub pd_uid: [u8; 0x10],
    pub package_version1: u16,
    pub package_version2: u16,
    pub package_version3: u16,
    pub package_version4: u16,
    pub pe_catalog_caps: [u16; 0x10],
    pub pe_catalogs: [u8; 0x80],
    pub writeable_expiration_date: u32,
    pub writeable_policy_flags: u32,
    pub persistent_local_storage_size: u32,
    pub mutable_data_page_count: u8,
    pub unknown271: u8,
    pub unknown272: [u8; 0x10],
    pub reserved282: [u8; 0xA],
    pub sequence_number: i64,
    pub unknown1: u16,
    pub unknown2: u16,
    pub unknown3: u16,
    pub unknown4: u16,
    pub odk_keyslot_id: MsiXVDOdkIndex,
    pub reserved2a0: [u8; 0xB54],
    pub resilient_data_offset: u64,
    pub resilient_data_length: u32,
}

impl MsiXVDHeader {
    pub fn mutable_data_length(&self) -> u64 {
        (self.mutable_data_page_count as u64) << 12
    }

    pub fn user_data_page_count(&self) -> u64 {
        (self.user_data_length as u64 + 0xFFF) >> 12
    }

    pub fn xvc_info_page_count(&self) -> u64 {
        (self.xvc_data_length as u64 + 0xFFF) >> 12
    }

    pub fn embedded_xvd_page_count(&self) -> u64 {
        (self.embedded_xvd_length as u64 + 0xFFF) >> 12
    }

    pub fn dynamic_header_page_count(&self) -> u64 {
        (self.dynamic_header_length as u64 + 0xFFF) >> 12
    }

    pub fn drive_page_count(&self) -> u64 {
        (self.drive_size + 0xFFF) >> 12
    }

    pub fn number_of_hashed_pages(&self) -> u64 {
        self.drive_page_count() + self.user_data_page_count() + self.xvc_info_page_count() + self.dynamic_header_page_count()
    }

    pub fn number_of_metadata_pages(&self) -> u64 {
        self.user_data_page_count() + self.xvc_info_page_count() + self.dynamic_header_page_count()
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ExtEntry {
    pub code: u32,
    pub length: u32,
    pub offset: u64,
    pub data_length: u32,
    pub reserved: u32,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXVDKind {
    Fixed = 0,
    Dynamic = 1,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXVDOdkIndex {
    StandardOdk = 0,
    GreenOdk = 1,
    RedOdk = 2,
    Invalid = 0xFFFFFFFF,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXVDContentCategory {
    Data = 0,
    Title = 1,
    SystemOS = 2,
    EraOS = 3,
    Scratch = 4,
    ResetData = 5,
    Application = 6,
    HostOS = 7,
    X360STFS = 8,
    X360FATX = 9,
    X360GDFX = 0xA,
    Updater = 0xB,
    OfflineUpdater = 0xC,
    Template = 0xD,
    MteHost = 0xE,
    MteApp = 0xF,
    MteTitle = 0x10,
    MteEraOS = 0x11,
    EraTools = 0x12,
    SystemTools = 0x13,
    SystemAux = 0x14,
    AcousticModel = 0x15,
    SystemCodecsVolume = 0x16,
    QasltPackage = 0x17,
    AppDlc = 0x18,
    TitleDlc = 0x19,
    UniversalDlc = 0x1A,
    SystemDataVolume = 0x1B,
    TestVolume = 0x1C,
    HardwareTestVolume = 0x1D,
    KioskContent = 0x1E,
    HostProfiler = 0x20,
    Uwa = 0x21,
    Unknown22 = 0x22,
    Unknown23 = 0x23,
    Unknown24 = 0x24,
    ServerAgent = 0x25,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXvcAreaAttributes {
    Resident = 1,
    InitialPlay = 2,
    Preview = 4,
    FileSystemMetadata = 8,
    Present = 0x10,
    OnDemand = 0x20,
    Available = 0x40,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXVDVolumeAttributes {
    ReadOnly = 1,
    EncryptionDisabled = 2,
    DataIntegrityDisabled = 4,
    LegacySectorSize = 8,
    ResiliencyEnabled = 0x10,
    SraReadOnly = 0x20,
    RegionIdInXts = 0x40,
    EraSpecific = 0x80,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXvcAreaPresenceInfo {
    IsPresent = 1,
    IsAvailable = 2,
    Disc1 = 0x10,
    Disc2 = 0x20,
    Disc3 = 0x30,
    Disc4 = 0x40,
    Disc5 = 0x50,
    Disc6 = 0x60,
    Disc7 = 0x70,
    Disc8 = 0x80,
    Disc9 = 0x90,
    Disc10 = 0xA0,
    Disc11 = 0xB0,
    Disc12 = 0xC0,
    Disc13 = 0xD0,
    Disc14 = 0xE0,
    Disc15 = 0xF0,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXVDUserDataCategory {
    PackageFiles = 0,
}

#[repr(u16)]
#[derive(Debug, Clone, Copy)]
pub enum MsiXVDSegmentMetadataFlags {
    KeepEncryptedOnDisk = 1,
}
