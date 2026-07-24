use sentinel_process::{ProcessEvent, SupervisedProcess};
use std::time::Duration;

#[tokio::test]
async fn captures_output_and_exit_code() {
    let (mut process, mut events) =
        SupervisedProcess::start("sh", &["-c", "echo out; echo err >&2; exit 0"])
            .await
            .unwrap();
    assert_eq!(process.wait().await.unwrap(), Some(0));
    let mut received = Vec::new();
    while let Ok(event) = events.try_recv() {
        received.push(event);
    }
    assert!(received
        .iter()
        .any(|event| matches!(event, ProcessEvent::Stdout(line) if line == "out")));
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_terminates_process_group() {
    let (mut process, _) = SupervisedProcess::start("sh", &["-c", "(trap '' TERM; while :; do sleep 1; done) & child=$!; echo $child; while :; do sleep 1; done"]).await.unwrap();
    let pid = process.pid();
    let result = process.cancel(Duration::from_millis(100)).await.unwrap();
    assert_ne!(result, Some(0));
    let alive = unsafe { libc::kill(-(pid as i32), 0) == 0 };
    assert!(!alive, "process group remains alive");
}
