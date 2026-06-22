/// 格式化字节大小为人类可读的字符串
///
/// 自动选择合适的单位（B, KB, MB, GB, TB），并保留两位小数
///
/// # 示例
/// ```
/// use crate::utils::format_bytes::format_bytes;
/// assert_eq!(format_bytes(0), "0 B");
/// assert_eq!(format_bytes(1024), "1.00 KB");
/// assert_eq!(format_bytes(1048576), "1.00 MB");
/// ```
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    match bytes {
        0 => "0 B".to_string(),
        b if b < KB => format!("{} B", b),
        b if b < MB => format!("{:.2} KB", b as f64 / KB as f64),
        b if b < GB => format!("{:.2} MB", b as f64 / MB as f64),
        b if b < TB => format!("{:.2} GB", b as f64 / GB as f64),
        b => format!("{:.2} TB", b as f64 / TB as f64),
    }
}

/// 格式化字节大小为紧凑形式（保留一位小数，用于空间有限的 UI）
pub fn format_bytes_compact(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    match bytes {
        0 => "0B".to_string(),
        b if b < KB => format!("{}B", b),
        b if b < MB => format!("{:.1}K", b as f64 / KB as f64),
        b if b < GB => format!("{:.1}M", b as f64 / MB as f64),
        b if b < TB => format!("{:.1}G", b as f64 / GB as f64),
        b => format!("{:.1}T", b as f64 / TB as f64),
    }
}

/// 格式化字节每秒的速度
pub fn format_bytes_per_sec(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 0.0 {
        return "-- B/s".to_string();
    }

    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    match bytes_per_sec {
        v if v < KB => format!("{:.0} B/s", v),
        v if v < MB => format!("{:.2} KB/s", v / KB),
        v if v < GB => format!("{:.2} MB/s", v / MB),
        _ => format!("{:.2} GB/s", bytes_per_sec / GB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
        assert_eq!(format_bytes(1572864), "1.50 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }

    #[test]
    fn test_format_bytes_compact() {
        assert_eq!(format_bytes_compact(0), "0B");
        assert_eq!(format_bytes_compact(512), "512B");
        assert_eq!(format_bytes_compact(1024), "1.0K");
        assert_eq!(format_bytes_compact(1048576), "1.0M");
        assert_eq!(format_bytes_compact(1073741824), "1.0G");
    }

    #[test]
    fn test_format_bytes_per_sec() {
        assert_eq!(format_bytes_per_sec(0.0), "0 B/s");
        assert_eq!(format_bytes_per_sec(512.0), "512 B/s");
        assert_eq!(format_bytes_per_sec(1024.0), "1.00 KB/s");
        assert_eq!(format_bytes_per_sec(1048576.0), "1.00 MB/s");
        assert_eq!(format_bytes_per_sec(-1.0), "-- B/s");
    }
}
