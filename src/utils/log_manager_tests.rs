use super::*;
use std::io::Read;

fn test_directory(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("bmcbl-log-manager-{name}-{}", uuid::Uuid::new_v4()))
}

#[test]
fn rotation_is_requested_for_size_or_date_boundary() {
    let day = NaiveDate::from_ymd_opt(2026, 7, 28).expect("valid test date");
    let next_day = NaiveDate::from_ymd_opt(2026, 7, 29).expect("valid test date");

    assert!(!should_rotate(0, 200, day, day, 100));
    assert!(!should_rotate(40, 60, day, day, 100));
    assert!(should_rotate(40, 61, day, day, 100));
    assert!(should_rotate(1, 1, day, next_day, 100));
}

#[test]
fn copy_file_tail_bounds_previous_log() -> io::Result<()> {
    let directory = test_directory("tail");
    fs::create_dir_all(&directory)?;
    let source = directory.join("latest.log");
    let destination = directory.join("previous.log");
    fs::write(&source, b"0123456789")?;

    copy_file_tail(&source, &destination, 4)?;

    assert_eq!(fs::read(&destination)?, b"6789");
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn pending_log_is_compressed_without_losing_content() -> io::Result<()> {
    let directory = test_directory("compress");
    fs::create_dir_all(&directory)?;
    let pending = directory.join("sample.log.pending");
    let contents = b"first line\nsecond line\n";
    fs::write(&pending, contents)?;

    archive_pending_file(&pending, 1)?;

    let archive = directory.join("sample.log.zst");
    let mut decoder = zstd::stream::read::Decoder::new(File::open(&archive)?)?;
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded)?;
    assert_eq!(decoded, contents);
    assert!(!pending.exists());
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn retention_uses_original_log_timestamp_from_archive_name() -> io::Result<()> {
    let directory = test_directory("retention");
    fs::create_dir_all(&directory)?;
    let old_archive = directory.join("bmcbl-001-1-00000000-00.log.zst");
    let now_millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("current time should follow the Unix epoch")
        .as_millis();
    let current_archive = directory.join(format!("bmcbl-{now_millis:013}-1-00000001-00.log.zst"));
    fs::write(&old_archive, b"old")?;
    fs::write(&current_archive, b"current")?;
    let policy = LogRetentionPolicy::from(&crate::config::config::LogManagementConfig::default());

    let removed = enforce_retention(&directory, policy)?;

    assert_eq!(removed, 1);
    assert!(!old_archive.exists());
    assert!(current_archive.exists());
    fs::remove_dir_all(directory)?;
    Ok(())
}
