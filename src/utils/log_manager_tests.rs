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
fn async_test_state(directory: &Path) -> io::Result<(Arc<Mutex<ActiveLog>>, PathBuf)> {
    let active_path = directory.join(ACTIVE_LOG_FILE);
    fs::create_dir_all(directory)?;
    fs::create_dir_all(directory.join(ARCHIVE_DIRECTORY))?;
    let file = open_active_log(&active_path)?;
    let policy = Arc::new(RwLock::new(LogRetentionPolicy {
        max_active_bytes: 1024 * 1024,
        max_previous_bytes: 1024,
        max_archive_bytes: 1024 * 1024,
        max_archive_files: 4,
        max_archive_age: Duration::from_secs(60),
        compression_level: 1,
    }));
    let state = Arc::new(Mutex::new(ActiveLog {
        path: active_path.clone(),
        archive_directory: directory.join(ARCHIVE_DIRECTORY),
        file: Some(file),
        bytes_written: 0,
        opened_on: Local::now().date_naive(),
        next_sequence: 1,
        policy,
        archive_wake: None,
    }));
    Ok((state, active_path))
}

fn send_log_event(sender: &SyncSender<LogMessage>, bytes: &[u8]) {
    sender
        .send(LogMessage::Bytes(bytes.to_vec()))
        .expect("test log queue should accept records");
}

#[test]
fn async_log_writer_persists_queued_events_in_order() -> io::Result<()> {
    let directory = test_directory("async-order");
    let (state, active_path) = async_test_state(&directory)?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(64);
    let dropped = Arc::new(AtomicU64::new(0));
    let worker_state = Arc::clone(&state);
    let worker_dropped = Arc::clone(&dropped);
    let writer = std::thread::spawn(move || run_log_writer(worker_state, receiver, worker_dropped));

    send_log_event(&sender, b"first record\n");
    send_log_event(&sender, b"second record\n");
    assert!(flush_log_sender(&sender, Duration::from_secs(2)));

    let content = fs::read_to_string(&active_path)?;
    assert!(content.contains("first record"));
    assert!(content.contains("second record"));
    assert!(
        content.find("first record") < content.find("second record"),
        "queued records must keep emission order"
    );

    drop(sender);
    writer.join().expect("log writer thread must exit");
    drop(state);
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn buffered_log_event_never_blocks_when_queue_is_full() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    sender
        .send(LogMessage::Bytes(b"occupied".to_vec()))
        .expect("first record should fit");
    let dropped = Arc::new(AtomicU64::new(0));

    let started_at = Instant::now();
    for _ in 0..128 {
        drop(BufferedLogEvent {
            sender: sender.clone(),
            dropped: Arc::clone(&dropped),
            bytes: b"dropped record".to_vec(),
        });
    }

    assert!(
        started_at.elapsed() < Duration::from_secs(1),
        "a saturated queue must never stall the emitting thread"
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 128);
    drop(receiver);
}

#[test]
fn log_flush_times_out_when_writer_is_not_draining() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(8);
    let started_at = Instant::now();
    assert!(!flush_log_sender(&sender, Duration::from_millis(50)));
    assert!(started_at.elapsed() >= Duration::from_millis(40));
    drop(receiver);
}

#[test]
fn saturated_queue_drops_are_reported_to_the_log_file() -> io::Result<()> {
    let directory = test_directory("async-dropped");
    let (state, active_path) = async_test_state(&directory)?;
    let dropped = Arc::new(AtomicU64::new(3));
    let mut batch = b"kept record\n".to_vec();

    write_log_batch(&state, &mut batch, &dropped);

    let content = fs::read_to_string(&active_path)?;
    assert!(content.contains("kept record"));
    assert!(content.contains("dropped 3 events"));
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    fs::remove_dir_all(directory)?;
    Ok(())
}
