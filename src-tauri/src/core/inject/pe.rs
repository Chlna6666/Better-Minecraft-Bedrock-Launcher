// src/core/inject/pe.rs
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use byteorder::{ReadBytesExt, WriteBytesExt, LittleEndian};
use tracing::{info, warn};

// --- PE 常量 ---
const IMAGE_DOS_SIGNATURE: u16 = 0x5A4D; // MZ
const IMAGE_NT_SIGNATURE: u32 = 0x00004550; // PE\0\0

const IMAGE_NT_OPTIONAL_HDR32_MAGIC: u16 = 0x10b;
const IMAGE_NT_OPTIONAL_HDR64_MAGIC: u16 = 0x20b;

const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;
const IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT: usize = 11;

const IMAGE_SCN_CNT_INITIALIZED_DATA: u32 = 0x00000040;
const IMAGE_SCN_MEM_READ: u32 = 0x40000000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x80000000;

// [序号导入标志位]
const IMAGE_ORDINAL_FLAG64: u64 = 0x8000000000000000;
const IMAGE_ORDINAL_FLAG32: u32 = 0x80000000;

const INJECT_SECTION_NAME: &[u8; 8] = b".nimp\0\0\0";
const INJECT_SECTION_NAME_STR: &str = ".nimp";

// --- BMCBL 自标记 (用于无需 exe.bak 还原 PE 头) ---
// 文件末尾追加:
//   [payload][payload_len:u32 LE][magic:8]
// payload:
//   [version:u32 LE][original_file_len:u64 LE][original_header_len:u32 LE][original_header_bytes...]
const BMCBL_PATCH_MAGIC: &[u8; 8] = b"BMCBLPE1";
const BMCBL_PATCH_VERSION: u32 = 1;
const BMCBL_PATCH_TRAILER_LEN: u64 = 12;

fn align_up(val: u32, align: u32) -> u32 {
    if align == 0 { val } else { (val + align - 1) & !(align - 1) }
}

pub fn is_file_patched(exe_path: &Path) -> bool {
    let Ok(mut file) = File::open(exe_path) else { return false; };
    if read_patch_marker(&mut file).ok().flatten().is_some() {
        return true;
    }
    has_inject_section_name(&mut file).unwrap_or(false)
}

fn has_inject_section_name(file: &mut File) -> io::Result<bool> {
    file.seek(SeekFrom::Start(0))?;

    let mut cursor = io::BufReader::new(file);
    let e_magic = cursor.read_u16::<LittleEndian>()?;
    if e_magic != IMAGE_DOS_SIGNATURE {
        return Ok(false);
    }
    cursor.seek(SeekFrom::Start(0x3C))?;
    let e_lfanew = cursor.read_u32::<LittleEndian>()? as u64;

    cursor.seek(SeekFrom::Start(e_lfanew))?;
    if cursor.read_u32::<LittleEndian>()? != IMAGE_NT_SIGNATURE {
        return Ok(false);
    }

    cursor.seek(SeekFrom::Current(2))?;
    let number_of_sections = cursor.read_u16::<LittleEndian>()?;
    cursor.seek(SeekFrom::Current(12))?;
    let size_of_optional_header = cursor.read_u16::<LittleEndian>()? as u64;
    cursor.seek(SeekFrom::Current(2))?;

    let first_section_off = e_lfanew + 4 + 20 + size_of_optional_header;
    cursor.seek(SeekFrom::Start(first_section_off))?;

    let mut name = [0u8; 8];
    for _ in 0..number_of_sections {
        cursor.read_exact(&mut name)?;
        if name == *INJECT_SECTION_NAME {
            return Ok(true);
        }
        cursor.seek(SeekFrom::Current(32))?; // rest of IMAGE_SECTION_HEADER
    }

    Ok(false)
}

#[derive(Debug, Clone)]
struct PatchMarker {
    version: u32,
    original_file_len: u64,
    original_header: Vec<u8>,
}

fn read_patch_marker(file: &mut File) -> io::Result<Option<PatchMarker>> {
    let file_len = file.metadata()?.len();
    if file_len < BMCBL_PATCH_TRAILER_LEN {
        return Ok(None);
    }

    file.seek(SeekFrom::End(-(BMCBL_PATCH_TRAILER_LEN as i64)))?;
    let payload_len = file.read_u32::<LittleEndian>()? as u64;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if magic != *BMCBL_PATCH_MAGIC {
        return Ok(None);
    }

    if file_len < BMCBL_PATCH_TRAILER_LEN + payload_len {
        return Ok(None);
    }

    let payload_start = file_len - BMCBL_PATCH_TRAILER_LEN - payload_len;
    file.seek(SeekFrom::Start(payload_start))?;
    let mut payload = vec![0u8; payload_len as usize];
    file.read_exact(&mut payload)?;

    let mut cur = Cursor::new(payload);
    let version = cur.read_u32::<LittleEndian>()?;
    if version != BMCBL_PATCH_VERSION {
        return Ok(None);
    }

    let original_file_len = cur.read_u64::<LittleEndian>()?;
    let original_header_len = cur.read_u32::<LittleEndian>()? as usize;

    let mut original_header = vec![0u8; original_header_len];
    cur.read_exact(&mut original_header)?;

    Ok(Some(PatchMarker {
        version,
        original_file_len,
        original_header,
    }))
}

fn append_patch_marker(final_buf: &mut Vec<u8>, original_file_len: u64, original_header: &[u8]) -> io::Result<()> {
    let mut payload = Vec::with_capacity(4 + 8 + 4 + original_header.len());
    payload.write_u32::<LittleEndian>(BMCBL_PATCH_VERSION)?;
    payload.write_u64::<LittleEndian>(original_file_len)?;
    payload.write_u32::<LittleEndian>(original_header.len() as u32)?;
    payload.extend_from_slice(original_header);

    final_buf.extend_from_slice(&payload);
    final_buf.write_u32::<LittleEndian>(payload.len() as u32)?;
    final_buf.extend_from_slice(BMCBL_PATCH_MAGIC);
    Ok(())
}

/// 向 PE 注入 DLL
///
/// - dll_name: 目标 DLL 名称 (如 "preloader.dll")
/// - export_name:
///     - Some("func_name"): 按名称导入
///     - None: 按序号(Ordinal 1)导入 -> 实现“只加载DLL不指定符号”
pub fn inject_dll_import(exe_path: &Path, dll_name: &str, export_name: Option<&str>) -> Result<(), String> {
    info!("PE Inject: 处理 {:?} -> 导入 {} (模式: {})",
        exe_path,
        dll_name,
        if export_name.is_some() { "ByName" } else { "ByOrdinal(1)" }
    );

    // 如果已被新版本标记，避免重复注入破坏文件结构
    if is_file_patched(exe_path) {
        warn!("PE Inject: 文件已包含补丁标记，跳过: {}", exe_path.display());
        return Ok(());
    }

    let mut file = OpenOptions::new().read(true).write(true).open(exe_path)
        .map_err(|e| format!("无法打开文件: {}", e))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).map_err(|e| e.to_string())?;
    let mut cursor = Cursor::new(buffer);
    let original_file_len = cursor.get_ref().len() as u64;

    // 1. DOS & PE Signature Check
    let e_magic = cursor.read_u16::<LittleEndian>().map_err(|e| e.to_string())?;
    if e_magic != IMAGE_DOS_SIGNATURE { return Err("无效 DOS 头".into()); }
    cursor.seek(SeekFrom::Start(0x3C)).unwrap();
    let e_lfanew = cursor.read_u32::<LittleEndian>().map_err(|e| e.to_string())? as u64;

    cursor.seek(SeekFrom::Start(e_lfanew)).unwrap();
    if cursor.read_u32::<LittleEndian>().map_err(|e| e.to_string())? != IMAGE_NT_SIGNATURE {
        return Err("无效 PE 签名".into());
    }

    // 2. File Header
    cursor.seek(SeekFrom::Current(2)).unwrap();
    let number_of_sections = cursor.read_u16::<LittleEndian>().unwrap();
    cursor.seek(SeekFrom::Current(12)).unwrap();
    let size_of_optional_header = cursor.read_u16::<LittleEndian>().unwrap();
    cursor.seek(SeekFrom::Current(2)).unwrap();

    // 3. Optional Header
    let opt_header_pos = cursor.position();
    let magic = cursor.read_u16::<LittleEndian>().unwrap();
    let is_64bit = match magic {
        IMAGE_NT_OPTIONAL_HDR64_MAGIC => true,
        IMAGE_NT_OPTIONAL_HDR32_MAGIC => false,
        _ => return Err(format!("未知 Magic: 0x{:x}", magic)),
    };

    let size_of_init_data_off = opt_header_pos + 8;
    let section_alignment_off = opt_header_pos + 32;
    let file_alignment_off = opt_header_pos + 36;
    let size_of_image_off = opt_header_pos + 56;
    let checksum_off = opt_header_pos + 64;
    let size_of_headers_off = opt_header_pos + 60;
    let data_dirs_off = if is_64bit { opt_header_pos + 112 } else { opt_header_pos + 96 };

    cursor.seek(SeekFrom::Start(section_alignment_off)).unwrap();
    let section_alignment = cursor.read_u32::<LittleEndian>().unwrap();
    let file_alignment = cursor.read_u32::<LittleEndian>().unwrap();

    cursor.seek(SeekFrom::Start(size_of_headers_off)).unwrap();
    let size_of_headers = cursor.read_u32::<LittleEndian>().unwrap();
    let header_len = (size_of_headers as usize).min(cursor.get_ref().len());
    let original_header = cursor.get_ref()[..header_len].to_vec();

    cursor.seek(SeekFrom::Start(size_of_image_off)).unwrap();
    let _size_of_image = cursor.read_u32::<LittleEndian>().unwrap();

    let import_dir_off = data_dirs_off + (IMAGE_DIRECTORY_ENTRY_IMPORT as u64 * 8);
    let bound_import_off = data_dirs_off + (IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT as u64 * 8);

    cursor.seek(SeekFrom::Start(import_dir_off)).unwrap();
    let old_import_rva = cursor.read_u32::<LittleEndian>().unwrap();

    // 4. 读取 Sections
    let first_section_off = opt_header_pos + size_of_optional_header as u64;
    struct Section { v_addr: u32, v_size: u32, raw_ptr: u32 }
    let mut sections = Vec::new();
    cursor.seek(SeekFrom::Start(first_section_off)).unwrap();
    for _ in 0..number_of_sections {
        cursor.seek(SeekFrom::Current(8)).unwrap();
        let v_size = cursor.read_u32::<LittleEndian>().unwrap();
        let v_addr = cursor.read_u32::<LittleEndian>().unwrap();
        let _raw_size = cursor.read_u32::<LittleEndian>().unwrap();
        let raw_ptr = cursor.read_u32::<LittleEndian>().unwrap();
        cursor.seek(SeekFrom::Current(16)).unwrap();
        sections.push(Section { v_addr, v_size, raw_ptr });
    }

    // 5. 读取旧导入表
    let mut imports = Vec::new();
    if old_import_rva > 0 {
        let section = sections.iter().find(|s| old_import_rva >= s.v_addr && old_import_rva < s.v_addr + s.v_size)
            .ok_or("找不到旧导入表")?;
        let offset = section.raw_ptr + (old_import_rva - section.v_addr);
        cursor.seek(SeekFrom::Start(offset as u64)).unwrap();
        loop {
            let ft = cursor.read_u32::<LittleEndian>().unwrap();
            let tds = cursor.read_u32::<LittleEndian>().unwrap();
            let fc = cursor.read_u32::<LittleEndian>().unwrap();
            let name = cursor.read_u32::<LittleEndian>().unwrap();
            let iat = cursor.read_u32::<LittleEndian>().unwrap();
            if ft == 0 && name == 0 { break; }
            imports.push((ft, tds, fc, name, iat));
        }
    }

    // 6. 构建新节 (.nimp)
    let last_sect = sections.last().ok_or("无节")?;
    let new_sect_rva = align_up(last_sect.v_addr + last_sect.v_size, section_alignment);
    let current_file_len = cursor.get_ref().len() as u32;
    let new_sect_offset = align_up(current_file_len, file_alignment);

    let mut new_data = Vec::new();

    // 布局: [NewDescriptor] [OldDescriptors...] [NullDescriptor]
    let descriptors_size = (imports.len() + 2) * 20;
    new_data.resize(descriptors_size, 0);

    // 复制旧导入表 (从索引 1 开始)
    for (i, imp) in imports.iter().enumerate() {
        let base = (i + 1) * 20;
        new_data[base..base+4].copy_from_slice(&imp.0.to_le_bytes());
        new_data[base+4..base+8].copy_from_slice(&imp.1.to_le_bytes());
        new_data[base+8..base+12].copy_from_slice(&imp.2.to_le_bytes());
        new_data[base+12..base+16].copy_from_slice(&imp.3.to_le_bytes());
        new_data[base+16..base+20].copy_from_slice(&imp.4.to_le_bytes());
    }

    // 写入 DLL Name
    let dll_name_rva = new_sect_rva + new_data.len() as u32;
    new_data.extend_from_slice(dll_name.as_bytes());
    new_data.push(0);
    while new_data.len() % 2 != 0 { new_data.push(0); }

    // 准备导入内容
    let mut thunk_value_64 = 0u64;
    let mut thunk_value_32 = 0u32;

    if let Some(exp_name) = export_name {
        // [方式A] 按名称导入
        let ibn_rva = new_sect_rva + new_data.len() as u32;
        new_data.write_u16::<LittleEndian>(0).unwrap(); // Hint
        new_data.extend_from_slice(exp_name.as_bytes());
        new_data.push(0);
        while new_data.len() % 8 != 0 { new_data.push(0); } // 8字节对齐

        thunk_value_64 = ibn_rva as u64;
        thunk_value_32 = ibn_rva as u32;
    } else {
        // [方式B] 按序号导入 (Ordinal 1)
        // 最高位设置为 1，低位为序号
        thunk_value_64 = IMAGE_ORDINAL_FLAG64 | 1;
        thunk_value_32 = IMAGE_ORDINAL_FLAG32 | 1;
    }

    // 写入 Thunk Table (ILT / IAT)
    let ilt_rva = new_sect_rva + new_data.len() as u32;
    if is_64bit {
        new_data.write_u64::<LittleEndian>(thunk_value_64).unwrap();
        new_data.write_u64::<LittleEndian>(0).unwrap();
    } else {
        new_data.write_u32::<LittleEndian>(thunk_value_32).unwrap();
        new_data.write_u32::<LittleEndian>(0).unwrap();
    }

    let iat_rva = new_sect_rva + new_data.len() as u32;
    if is_64bit {
        new_data.write_u64::<LittleEndian>(thunk_value_64).unwrap();
        new_data.write_u64::<LittleEndian>(0).unwrap();
    } else {
        new_data.write_u32::<LittleEndian>(thunk_value_32).unwrap();
        new_data.write_u32::<LittleEndian>(0).unwrap();
    }

    // 填回新导入表描述符 (索引 0)
    let slice = &mut new_data[0..20];
    slice[0..4].copy_from_slice(&ilt_rva.to_le_bytes()); // OriginalFirstThunk
    slice[12..16].copy_from_slice(&dll_name_rva.to_le_bytes()); // Name
    slice[16..20].copy_from_slice(&iat_rva.to_le_bytes()); // FirstThunk

    // 7. 写入文件
    let raw_size = align_up(new_data.len() as u32, file_alignment);
    let current_len = cursor.get_ref().len();
    let pad_len = new_sect_offset as usize - current_len;

    let mut final_buf = cursor.into_inner();
    final_buf.extend(std::iter::repeat(0).take(pad_len)); // 文件对齐
    final_buf.extend(&new_data);
    let data_pad_len = raw_size as usize - new_data.len();
    final_buf.extend(std::iter::repeat(0).take(data_pad_len)); // 节区对齐

    let mut cursor = Cursor::new(final_buf);

    // 8. 更新头信息
    cursor.seek(SeekFrom::Start(e_lfanew + 6)).unwrap();
    cursor.write_u16::<LittleEndian>(number_of_sections + 1).unwrap();

    cursor.seek(SeekFrom::Start(size_of_init_data_off)).unwrap();
    let old_size_init = cursor.read_u32::<LittleEndian>().unwrap();
    cursor.seek(SeekFrom::Start(size_of_init_data_off)).unwrap();
    cursor.write_u32::<LittleEndian>(old_size_init + raw_size).unwrap();

    let new_img_size = align_up(new_sect_rva + align_up(new_data.len() as u32, section_alignment), section_alignment);
    cursor.seek(SeekFrom::Start(size_of_image_off)).unwrap();
    cursor.write_u32::<LittleEndian>(new_img_size).unwrap();

    // 指向新导入表
    cursor.seek(SeekFrom::Start(import_dir_off)).unwrap();
    cursor.write_u32::<LittleEndian>(new_sect_rva).unwrap();
    cursor.write_u32::<LittleEndian>(descriptors_size as u32).unwrap();

    // 清除 Bound Import
    cursor.seek(SeekFrom::Start(bound_import_off)).unwrap();
    cursor.write_u32::<LittleEndian>(0).unwrap();
    cursor.write_u32::<LittleEndian>(0).unwrap();

    // 清除 Checksum
    cursor.seek(SeekFrom::Start(checksum_off)).unwrap();
    cursor.write_u32::<LittleEndian>(0).unwrap();

    // 9. 写入新节头
    let section_headers_offset = first_section_off + (number_of_sections as u64 * 40);
    cursor.seek(SeekFrom::Start(section_headers_offset)).unwrap();
    let mut name_bytes = [0u8; 8];
    name_bytes.copy_from_slice(INJECT_SECTION_NAME);
    cursor.write_all(&name_bytes).unwrap();
    cursor.write_u32::<LittleEndian>(new_data.len() as u32).unwrap(); // VirtualSize
    cursor.write_u32::<LittleEndian>(new_sect_rva).unwrap(); // VirtualAddress
    cursor.write_u32::<LittleEndian>(raw_size).unwrap(); // SizeOfRawData
    cursor.write_u32::<LittleEndian>(new_sect_offset).unwrap(); // PointerToRawData
    cursor.write_u32::<LittleEndian>(0).unwrap();
    cursor.write_u32::<LittleEndian>(0).unwrap();
    cursor.write_u16::<LittleEndian>(0).unwrap();
    cursor.write_u16::<LittleEndian>(0).unwrap();
    cursor.write_u32::<LittleEndian>(IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE).unwrap();

    // 10. 追加 BMCBL 自标记：允许无需 exe.bak 还原原始 PE 头
    if let Err(e) = append_patch_marker(cursor.get_mut(), original_file_len, &original_header) {
        warn!("写入 BMCBL PE 标记失败 (不影响注入): {}", e);
    }

    // 保存
    file.seek(SeekFrom::Start(0)).unwrap();
    file.set_len(0).unwrap();
    file.write_all(cursor.get_ref()).map_err(|e| e.to_string())?;

    info!("PE Patch 完成: 序号导入 (Ordinal 1).");
    Ok(())
}

pub fn ensure_backup(exe_path: &Path) -> Result<(), String> {
    let bak = exe_path.with_extension("exe.bak");
    if !bak.exists() {
        fs::copy(exe_path, &bak).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn restore_original_pe(exe_path: &Path) -> Result<(), String> {
    if try_restore_from_marker(exe_path).map_err(|e| e.to_string())? {
        return Ok(());
    }
    let bak = exe_path.with_extension("exe.bak");
    if bak.exists() {
        if exe_path.exists() { fs::remove_file(exe_path).map_err(|e| e.to_string())?; }
        fs::copy(&bak, exe_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn try_restore_from_marker(exe_path: &Path) -> io::Result<bool> {
    let mut file = OpenOptions::new().read(true).write(true).open(exe_path)?;
    let Some(marker) = read_patch_marker(&mut file)? else {
        return Ok(false);
    };

    let current_len = file.metadata()?.len();
    if marker.original_file_len == 0 || marker.original_file_len > current_len {
        return Ok(false);
    }
    if marker.original_header.is_empty() || (marker.original_header.len() as u64) > marker.original_file_len {
        return Ok(false);
    }

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&marker.original_header)?;
    file.set_len(marker.original_file_len)?;
    file.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_file(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let pid = std::process::id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        p.push(format!("bmcb_pe_test_{}_{}_{}_{}", name, pid, now, "x.exe"));
        p
    }

    #[test]
    fn marker_roundtrip_and_restore() {
        let exe_path = unique_temp_file("restore");
        let original = (0u8..=255).cycle().take(4096).collect::<Vec<u8>>();
        fs::write(&exe_path, &original).unwrap();

        // 模拟“打补丁后”的文件：原始内容 + 额外数据 + marker
        let mut patched = original.clone();
        patched.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        append_patch_marker(&mut patched, original.len() as u64, &original[..512]).unwrap();
        fs::write(&exe_path, &patched).unwrap();

        // 还原应当回到原始长度与头部数据
        assert!(try_restore_from_marker(&exe_path).unwrap());
        let restored = fs::read(&exe_path).unwrap();
        assert_eq!(restored.len(), original.len());
        assert_eq!(&restored[..512], &original[..512]);

        let _ = fs::remove_file(&exe_path);
    }
}
