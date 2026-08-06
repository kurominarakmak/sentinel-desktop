use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("codex 0.test");
        return;
    }
    if std::env::var("SENTINEL_FAKE_CODEX_SCENARIO")
        .ok()
        .as_deref()
        == Some("exit")
    {
        std::process::exit(17);
    }
    let stdin = io::stdin();
    let mut output = io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                println!("not-json");
                continue;
            }
        };
        let id = value.get("id").cloned();
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method == "initialized" {
            continue;
        }
        if std::env::var("SENTINEL_FAKE_CODEX_SCENARIO")
            .ok()
            .as_deref()
            == Some("malformed")
        {
            println!("not-json");
            let _ = output.flush();
            continue;
        }
        if let Some(id) = id {
            if method == "turn/start" {
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"delta":"hello"}})
                );
            }
            let result = match method {
                "initialize" => json!({"protocolVersion":"1"}),
                "thread/start" | "thread/resume" => json!({"thread":{"id":"thread-test"}}),
                "turn/start" => json!({"turn":{"id":"turn-test"}}),
                "turn/interrupt" => json!({}),
                _ => json!({}),
            };
            println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
            let _ = output.flush();
        }
    }
}
