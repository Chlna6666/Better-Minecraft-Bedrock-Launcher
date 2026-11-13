use super::LaunchTask;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;

#[tokio::test]
async fn launch_reports_pid_before_waiting_for_final_statistics() -> Result<(), String> {
    let (completion_tx, completion) = oneshot::channel();
    let (monitor_tx, session_monitor) = oneshot::channel();
    let (exit_tx, exit_rx) = oneshot::channel::<()>();
    let saved = Arc::new(AtomicBool::new(false));
    let saved_by_monitor = Arc::clone(&saved);
    let monitor = tokio::spawn(async move {
        let _ = exit_rx.await;
        saved_by_monitor.store(true, Ordering::SeqCst);
    });
    assert!(monitor_tx.send(Some(monitor)).is_ok());
    assert!(completion_tx.send(Ok(Some(42))).is_ok());
    let mut launch = LaunchTask {
        task_id: "test-launch".to_string(),
        completion,
        session_monitor,
        spawn_error: None,
    };

    assert_eq!(launch.wait().await?, Some(42));
    let finished = launch.wait_for_session();
    tokio::pin!(finished);
    assert!(futures::poll!(&mut finished).is_pending());
    assert!(!saved.load(Ordering::SeqCst));
    assert!(exit_tx.send(()).is_ok());
    finished.await?;
    assert!(saved.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn launch_without_session_monitor_finishes_immediately() -> Result<(), String> {
    let (completion_tx, completion) = oneshot::channel();
    let (monitor_tx, session_monitor) = oneshot::channel();
    assert!(completion_tx.send(Ok(None)).is_ok());
    assert!(monitor_tx.send(None).is_ok());
    let mut launch = LaunchTask {
        task_id: "test-launch".to_string(),
        completion,
        session_monitor,
        spawn_error: None,
    };

    assert_eq!(launch.wait().await?, None);
    launch.wait_for_session().await
}
