use std::{env, thread, time::Duration};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("fake-claude 1.0.0");
        return;
    }
    let scenario = env::var("SENTINEL_FAKE_CLAUDE_SCENARIO").unwrap_or_else(|_| "stream".into());
    let session = args
        .iter()
        .position(|arg| arg == "--resume")
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| "claude-session-test".into());
    println!("{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{session}\"}}");
    if scenario == "malformed" {
        println!("not json");
    }
    if scenario == "cancel" {
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if scenario != "exit" {
        println!(
            "{}",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#
        );
        println!("{}", r#"{"type":"tool_use","name":"Read"}"#);
        println!(
            "{}",
            r#"{"type":"result","subtype":"success","result":"done"}"#
        );
    }
}
