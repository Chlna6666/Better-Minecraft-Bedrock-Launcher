use std::fs::{self, File};
use std::io::{BufWriter, Write, Seek, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::HashSet;
use tracing::{debug, error, info, warn};
use rayon::prelude::*;
use uuid::Uuid;
use tokio::runtime::Handle; // [新增] 引入 Tokio Handle

#[cfg(windows)]
use std::os::windows::fs::FileExt;
#[cfg(unix)]
use std::os::unix::fs::FileExt;

use crate::tasks::task_manager::{is_cancelled, update_progress, set_total};

use super::decoder::MsiXVDDecoder;
use super::header::{MsiXVDHeader, MsiXVDVolumeAttributes, MsiXVDKind};
use super::key::CikKey;
use super::structs::*;

const XVD_HEADER_INCL_SIGNATURE_SIZE: u64 = 0x3000;

const RELEASE_GUID_STR: &str = "bdb9e791-c97c-3734-e1a8-bc602552df06";
const PRE_RELEASE_GUID_STR: &str = "1f49d63f-8bf5-1f8d-ed7e-dbd89477dad9";

// [修改方案] 使用 include_bytes! 宏将密钥硬编码进二进制文件
fn get_release_key_bytes() -> Option<Vec<u8>> {
    // 1. 优先尝试运行时读取环境变量（方便调试或覆盖）
    if let Ok(hex_str) = std::env::var("GDK_RELEASE_KEY") {
        if let Ok(bytes) = hex::decode(hex_str) {
            return Some(bytes);
        }
    }

    // 2. 编译时直接嵌入文件内容 (最稳健)
    // 注意：路径是相对于当前 .rs 源文件的，或者项目根目录（取决于宏的实现，include_bytes 通常基于 Cargo.toml 所在目录）
    // 假设你的目录结构是标准的，这里尝试硬编码嵌入
    const RELEASE_KEY: &[u8] = include_bytes!("../../../core/minecraft/gdk/Cik/bdb9e791-c97c-3734-e1a8-bc602552df06.cik");

    // 如果上面的相对路径报错，请根据此文件相对于 .cik 文件的位置调整路径
    // 比如如果当前文件在 src/utils/xxx.rs，你需要用 ../ 回退到 src 目录

    Some(RELEASE_KEY.to_vec())
}

fn get_pre_release_key_bytes() -> Option<Vec<u8>> {
    if let Ok(hex_str) = std::env::var("GDK_PREVIEW_KEY") {
        if let Ok(bytes) = hex::decode(hex_str) {
            return Some(bytes);
        }
    }

    const PREVIEW_KEY: &[u8] = include_bytes!("../../../core/minecraft/gdk/Cik/1f49d63f-8bf5-1f8d-ed7e-dbd89477dad9.cik");
    Some(PREVIEW_KEY.to_vec())
}
// --- 基础 IO 封装 ---

#[cfg(windows)]
fn read_at_impl(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    file.seek_read(buf, offset)
}

#[cfg(unix)]
fn read_at_impl(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    file.read_at(buf, offset)
}

/// 强制循环读取直到填满缓冲区
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        match read_at_impl(file, buf, offset) {
            Ok(0) => break, // EOF
            Ok(n) => {
                let tmp = buf;
                buf = &mut tmp[n..];
                offset += n as u64;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    if !buf.is_empty() {
        Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "failed to fill whole buffer"))
    } else {
        Ok(())
    }
}

unsafe fn read_struct_at<T: Copy>(buffer: &[u8], offset: usize) -> Result<T, String> {
    let size = std::mem::size_of::<T>();
    if offset + size > buffer.len() {
        return Err(format!("读取越界: 需要 {} 字节，偏移 {}，总长 {}", size, offset, buffer.len()));
    }
    Ok(std::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const T))
}

pub struct MsiXVDStream {
    file: File,
    header: MsiXVDHeader,
    is_encrypted: bool,
    segments: Vec<SegmentsAbout>,
    segment_paths: Vec<String>,
    xvc_regions: Vec<XvcRegionHeader>,
    xvc_update_segments: Vec<XvcUpdateSegment>,
    encryption_key_ids: Vec<Uuid>,
    xvd_user_data_offset: u64,
    hash_tree_page_offset: u64,
    hash_tree_levels: u64,
    data_integrity: bool,
    resiliency: bool,
}

struct ExtractJob {
    input_offset: u64,
    file_size: u64,
    output_path: PathBuf,
    base_iv: [u8; 16],
    should_decrypt: bool,
    start_block_index: u64,
}

impl MsiXVDStream {
    pub fn new(file_path: &Path) -> Result<Self, String> {
        info!("正在解析 GDK 文件结构: {:?}", file_path);
        let mut file = File::open(file_path).map_err(|e| format!("无法打开文件: {}", e))?;

        let header = Self::parse_file_header(&mut file)?;
        let volume_flags = header.volumes as u32;
        let is_encrypted = (volume_flags & (MsiXVDVolumeAttributes::EncryptionDisabled as u32)) == 0;
        let resiliency = (volume_flags & (MsiXVDVolumeAttributes::ResiliencyEnabled as u32)) != 0;
        let data_integrity = (volume_flags & (MsiXVDVolumeAttributes::DataIntegrityDisabled as u32)) == 0;

        let (hash_tree_page_count, hash_tree_levels) = Self::calculate_number_hash_pages(header.number_of_hashed_pages(), resiliency);
        let mutable_data_offset = (header.embedded_xvd_page_count() << 12) + XVD_HEADER_INCL_SIGNATURE_SIZE;
        let hash_tree_page_offset = header.mutable_data_length() + mutable_data_offset;

        let xvd_user_data_offset = if data_integrity {
            hash_tree_page_offset + (hash_tree_page_count << 12)
        } else {
            hash_tree_page_offset
        };

        let mut stream = Self {
            file, header, is_encrypted, segments: Vec::new(), segment_paths: Vec::new(),
            xvc_regions: Vec::new(), xvc_update_segments: Vec::new(),
            encryption_key_ids: Vec::new(),
            xvd_user_data_offset, hash_tree_page_offset, hash_tree_levels,
            data_integrity, resiliency,
        };

        stream.parse_user_data()?;
        stream.parse_area_info()?;
        Ok(stream)
    }

    fn select_cik(&self) -> Result<CikKey, String> {
        let mut candidates: Vec<(Option<Vec<u8>>, &str, &str)> = Vec::new();

        candidates.push((get_release_key_bytes(), RELEASE_GUID_STR, "Release"));
        candidates.push((get_pre_release_key_bytes(), PRE_RELEASE_GUID_STR, "Preview"));

        for file_key_id in &self.encryption_key_ids {
            for (key_bytes_opt, guid_str, name) in candidates.iter() {
                if let Ok(expected_guid) = Uuid::parse_str(guid_str) {
                    if *file_key_id == expected_guid {
                        if let Some(key_bytes) = key_bytes_opt {
                            info!("匹配到密钥: {} ({})", name, guid_str);
                            return CikKey::find_and_create(key_bytes, guid_str).map_err(|e| e.to_string());
                        } else {
                            warn!("检测到 ID 匹配 ({})，但未找到对应的本地密钥文件或环境变量！", name);
                        }
                    }
                }
            }
        }

        warn!("未能在已知库中找到匹配的 KeyID，尝试回退...");
        let (fallback_key_opt, fallback_guid) = if self.header.package_version2 == 0 {
            (get_pre_release_key_bytes(), PRE_RELEASE_GUID_STR)
        } else {
            (get_release_key_bytes(), RELEASE_GUID_STR)
        };

        if let Some(key_bytes) = fallback_key_opt {
            CikKey::find_and_create(&key_bytes, fallback_guid).map_err(|e| e.to_string())
        } else {
            Err("未找到任何可用的 CIK 密钥".to_string())
        }
    }

    // [修改] 增加 task_id 参数，支持进度和取消
    pub fn extract_to(&mut self, output_dir: &Path, task_id: String) -> Result<(), String> {
        info!("开始提取文件到: {:?}", output_dir);
        fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;

        // [关键修改 1] 获取当前的 Tokio Runtime Handle
        let rt_handle = Handle::current();

        let cik = self.select_cik()?;
        let decoder = MsiXVDDecoder::new(&cik);

        let first_segment_offset = if !self.xvc_update_segments.is_empty() {
            (self.xvc_update_segments[0].page_num as u64) << 12
        } else { 0 };

        let mut jobs = Vec::new();
        let extractable_regions: Vec<_> = self.xvc_regions.iter().filter(|r| {
            r.first_segment_index != 0 || (first_segment_offset > 0 && first_segment_offset == r.offset)
        }).collect();

        for region in extractable_regions {
            let mut base_iv = [0u8; 16];
            let should_decrypt = self.is_encrypted && region.key_id != 0xFFFF;
            if should_decrypt {
                base_iv[4..8].copy_from_slice(&(region.id as u32).to_le_bytes());
                base_iv[8..16].copy_from_slice(&self.header.vd_uid[0..8]);
            }

            let mut current_offset = region.offset;
            let mut remaining_pages = region.length >> 12;
            let mut seg_idx = region.first_segment_index as usize;

            while seg_idx < self.segments.len() && remaining_pages > 0 {
                let segment = &self.segments[seg_idx];

                let seg_pages = if segment.file_size == 0 {
                    1
                } else {
                    (segment.file_size + 0xFFF) / 0x1000
                };

                let pages_to_process = seg_pages.min(remaining_pages);

                let segment_relative_offset = current_offset.saturating_sub(self.xvd_user_data_offset);
                let start_block_index = segment_relative_offset / 0x1000;

                jobs.push(ExtractJob {
                    input_offset: current_offset,
                    file_size: segment.file_size,
                    output_path: output_dir.join(&self.segment_paths[seg_idx]),
                    base_iv,
                    should_decrypt,
                    start_block_index,
                });

                current_offset += pages_to_process * 0x1000;
                remaining_pages = remaining_pages.saturating_sub(pages_to_process);
                seg_idx += 1;
            }
        }

        // 计算总大小并更新 task_manager
        let total_size: u64 = jobs.iter().map(|j| j.file_size).sum();
        set_total(&task_id, Some(total_size));

        // 如果一开始就取消了
        if is_cancelled(&task_id) {
            return Err("cancelled".to_string());
        }

        let total_jobs = jobs.len();
        let finished_counter = AtomicUsize::new(0);

        let parents: HashSet<_> = jobs.iter().filter_map(|job| job.output_path.parent()).collect();
        parents.par_iter().for_each(|path| { if !path.exists() { let _ = fs::create_dir_all(path); } });

        let file_ref = &self.file;
        let hash_tree_params = HashTreeParams {
            kind: self.header.kind, levels: self.hash_tree_levels,
            total_hashed_pages: self.header.number_of_hashed_pages(),
            resiliency: self.resiliency, tree_offset: self.hash_tree_page_offset,
            is_encrypted: self.is_encrypted, data_integrity: self.data_integrity,
        };

        const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB Buffer

        let task_id_ref = &task_id;

        // [关键修改 2] 将 rt_handle 传入并行迭代器
        jobs.par_iter().try_for_each_init(
            || (vec![0u8; CHUNK_SIZE], vec![0u8; CHUNK_SIZE], vec![0u8; 0x1000]),
            |(buffer, decrypt_buffer, hash_page_cache), job| -> Result<(), String> {
                // 在开始任务前检查取消
                if is_cancelled(task_id_ref) {
                    return Err("cancelled".to_string());
                }

                // 传入 &rt_handle
                if let Err(e) = Self::process_job(file_ref, job, &decoder, &hash_tree_params, buffer, decrypt_buffer, hash_page_cache, task_id_ref, &rt_handle) {
                    // 如果是 IO 错误但包含 Cancelled，或者显式 cancelled
                    if e.to_string().contains("cancelled") || is_cancelled(task_id_ref) {
                        return Err("cancelled".to_string());
                    }
                    error!("提取失败 {:?}: {}", job.output_path, e);
                    return Err(e.to_string());
                }

                let finished = finished_counter.fetch_add(1, Ordering::Relaxed) + 1;
                if finished % 100 == 0 { info!("进度: {} / {}", finished, total_jobs); }
                Ok(())
            }
        )?;

        // 最终检查
        if is_cancelled(&task_id) {
            return Err("cancelled".to_string());
        }

        Ok(())
    }

    // [修改] 增加 task_id 和 rt_handle 参数
    fn process_job(
        file: &File, job: &ExtractJob, decoder: &MsiXVDDecoder, hash_params: &HashTreeParams,
        buffer: &mut Vec<u8>, decrypt_buffer: &mut Vec<u8>, hash_page_cache: &mut Vec<u8>,
        task_id: &str,
        rt: &Handle // [新增参数]
    ) -> std::io::Result<()> {
        // [关键修改 3] 在函数入口处进入 Tokio 上下文
        // 这样当前 Rayon 线程在执行 update_progress 里的 tokio::spawn 时就能找到 Runtime 了
        let _guard = rt.enter();

        let output_file = File::create(&job.output_path)?;

        if job.file_size == 0 {
            return Ok(());
        }

        let mut writer = BufWriter::new(output_file);

        let input_aligned_size = ((job.file_size + 0xFFF) / 0x1000) * 0x1000;
        let mut remaining = input_aligned_size;
        let mut file_offset = job.input_offset;
        let mut bytes_written_total = 0;
        let mut current_block_index = job.start_block_index;
        let mut cached_hash_page_idx = u64::MAX;

        while remaining > 0 {
            // 循环内检查取消
            if is_cancelled(task_id) {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }

            let chunk_size = buffer.len().min(remaining as usize);
            let current_buf = &mut buffer[..chunk_size];

            read_exact_at(file, current_buf, file_offset)?;

            let data_to_write = if job.should_decrypt {
                let pages_in_chunk = chunk_size / 0x1000;
                let out_buf = &mut decrypt_buffer[..chunk_size];
                let mut iv = job.base_iv;

                for i in 0..pages_in_chunk {
                    let start = i * 0x1000;
                    let end = start + 0x1000;

                    if hash_params.data_integrity {
                        let (hash_page_idx, entry_idx) = Extensions::compute_hash_block_index(
                            hash_params.kind, hash_params.levels, hash_params.total_hashed_pages,
                            current_block_index + i as u64, 0, hash_params.resiliency
                        );

                        if hash_page_idx != cached_hash_page_idx {
                            read_exact_at(file, hash_page_cache, hash_params.tree_offset + (hash_page_idx * 0x1000))?;
                            cached_hash_page_idx = hash_page_idx;
                        }

                        let entry_len = if hash_params.is_encrypted { 20 } else { 24 };
                        let entry_offset = (entry_idx as usize) * 24;

                        if entry_offset + entry_len + 4 <= hash_page_cache.len() {
                            let src = &hash_page_cache[entry_offset + entry_len .. entry_offset + entry_len + 4];
                            iv[0..4].copy_from_slice(src);
                        }
                    }
                    unsafe { decoder.decrypt(&current_buf[start..end], &mut out_buf[start..end], &iv); }
                }
                out_buf
            } else {
                current_buf
            };

            let write_len = if bytes_written_total + (chunk_size as u64) > job.file_size {
                (job.file_size - bytes_written_total) as usize
            } else {
                chunk_size
            };

            writer.write_all(&data_to_write[..write_len])?;

            // 上报进度 (现在有了 _guard，这里的 tokio::spawn 不会 panic 了)
            update_progress(task_id, write_len as u64, None, None);

            remaining -= chunk_size as u64;
            file_offset += chunk_size as u64;
            bytes_written_total += chunk_size as u64;
            current_block_index += (chunk_size / 0x1000) as u64;
        }
        writer.flush()?;
        Ok(())
    }

    fn parse_file_header(file: &mut File) -> Result<MsiXVDHeader, String> {
        let mut header_bytes = vec![0u8; std::mem::size_of::<MsiXVDHeader>()];
        file.seek(std::io::SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        file.read_exact(&mut header_bytes).map_err(|e| e.to_string())?;
        unsafe { read_struct_at(&header_bytes, 0) }
    }

    fn parse_user_data(&mut self) -> Result<(), String> {
        self.file.seek(std::io::SeekFrom::Start(self.xvd_user_data_offset)).map_err(|e| e.to_string())?;
        let mut user_data_buffer = vec![0u8; self.header.user_data_length as usize];
        self.file.read_exact(&mut user_data_buffer).map_err(|e| e.to_string())?;

        let user_data_header: UserDataHeader = unsafe { read_struct_at(&user_data_buffer, 0)? };

        let data_type = user_data_header.data_type;

        if data_type == UserDataType::PackageFiles {
            let files_header: UserDataPackageFilesHeader = unsafe { read_struct_at(&user_data_buffer, user_data_header.length as usize)? };
            let entries_offset = user_data_header.length as usize + std::mem::size_of::<UserDataPackageFilesHeader>();

            for i in 0..files_header.file_count as usize {
                let entry: UserDataPackageFileEntry = unsafe { read_struct_at(&user_data_buffer, entries_offset + i * std::mem::size_of::<UserDataPackageFileEntry>())? };

                let raw_path = entry.file_path;

                let path_len = raw_path.iter().position(|&c| c == 0).unwrap_or(raw_path.len());
                let path = String::from_utf16_lossy(&raw_path[..path_len]);

                if path == "SegmentMetadata.bin" {
                    let offset = user_data_header.length as usize + entry.offset as usize;
                    return self.parse_segment_metadata(&user_data_buffer[offset..offset + entry.size as usize]);
                }
            }
            Ok(())
        } else {
            Err("Unsupported UserData type".into())
        }
    }

    fn parse_area_info(&mut self) -> Result<(), String> {
        let offset = self.xvd_user_data_offset + (self.header.user_data_page_count() << 12);
        self.file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; self.header.xvc_data_length as usize];
        self.file.read_exact(&mut buf).map_err(|e| e.to_string())?;

        let info: XvcInfo = unsafe { read_struct_at(&buf, 0)? };

        for key_id_struct in info.encryption_key_ids.iter() {
            let uuid = key_id_struct.as_uuid();
            if !uuid.is_nil() {
                self.encryption_key_ids.push(uuid);
            }
        }

        let mut curr = std::mem::size_of::<XvcInfo>();

        for _ in 0..info.region_count {
            self.xvc_regions.push(unsafe { read_struct_at(&buf, curr)? });
            curr += std::mem::size_of::<XvcRegionHeader>();
        }
        for _ in 0..info.update_segment_count {
            self.xvc_update_segments.push(unsafe { read_struct_at(&buf, curr)? });
            curr += std::mem::size_of::<XvcUpdateSegment>();
        }
        Ok(())
    }

    fn parse_segment_metadata(&mut self, data: &[u8]) -> Result<(), String> {
        let header: SegmentMetadataHeader = unsafe { read_struct_at(data, 0)? };
        let mut curr = std::mem::size_of::<SegmentMetadataHeader>();
        for _ in 0..header.segment_count {
            self.segments.push(unsafe { read_struct_at(data, curr)? });
            curr += std::mem::size_of::<SegmentsAbout>();
        }

        let paths_start = header.header_length as usize + header.segment_count as usize * 0x10;
        for seg in &self.segments {
            let start = paths_start + seg.path_offset as usize;
            let end = start + seg.path_length as usize * 2;
            let path_u16: Vec<u16> = data[start..end].chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            self.segment_paths.push(String::from_utf16_lossy(&path_u16).trim_matches('\0').to_string());
        }
        Ok(())
    }

    fn calculate_number_hash_pages(count: u64, resilient: bool) -> (u64, u64) {
        let mut pages = (count + 169) / 170;
        let mut levels = 1;
        if pages > 1 {
            let mut res = 2;
            while res > 1 {
                res = match levels {
                    1 => (count + 28900 - 1) / 28900,
                    2 => (count + 4913000 - 1) / 4913000,
                    _ => 0
                };
                if res > 0 { levels += 1; pages += res; } else { break; }
            }
        }
        if resilient { pages *= 2; }
        (pages, levels)
    }
}


struct HashTreeParams {
    kind: MsiXVDKind, levels: u64, total_hashed_pages: u64, resiliency: bool,
    tree_offset: u64, is_encrypted: bool, data_integrity: bool,
}

struct Extensions;
impl Extensions {
    fn compute_hash_block_index(
        image_type: MsiXVDKind, mut depth: u64, total: u64, idx: u64, level: u32, resilient: bool
    ) -> (u64, u64) {
        fn mult(l: u32) -> u64 { 170u64.pow(l) } // 0xAA
        if (image_type as u32) > 1 || level > 3 { return (0xFFFF, 0); }

        let entry_idx = if level == 0 { idx % 170 } else { (idx / mult(level)) % 170 };
        if level == 3 { return (0, entry_idx); }

        let mut block_idx = idx / mult(level + 1);
        depth -= (level + 1) as u64;

        if level == 0 && depth > 0 { block_idx += (total + mult(2) - 1) / mult(2); depth -= 1; }
        if (level <= 1) && depth > 0 { block_idx += (total + mult(3) - 1) / mult(3); depth -= 1; }
        if depth > 0 { block_idx += (total + mult(4) - 1) / mult(4); }
        if resilient { block_idx *= 2; }
        (block_idx, entry_idx)
    }
}