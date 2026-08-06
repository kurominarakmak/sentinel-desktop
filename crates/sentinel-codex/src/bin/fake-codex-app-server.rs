use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("codex 0.test");
        return;
    }
    let scenario = std::env::var("SENTINEL_FAKE_CODEX_SCENARIO").unwrap_or_default();
    if scenario == "exit" {
        std::process::exit(17);
    }
    let stdin = io::stdin();
    let mut output = io::stdout();
    let mut deferred = None;
    let mut non_initialize_requests = 0;
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
        if scenario == "malformed" {
            println!("not-json");
            let _ = output.flush();
            continue;
        }
        if let Some(id) = id {
            if method != "initialize" {
                non_initialize_requests += 1;
            }
            if scenario == "exit-during-request" && method != "initialize" {
                std::process::exit(17);
            }
            if scenario == "timeout-one" && method != "initialize" && non_initialize_requests == 1 {
                continue;
            }
            if scenario == "reverse" && method == "thread/start" {
                if let Some(first) = deferred.take() {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","id":id,"result":{"thread":{"id":"thread-second"}}})
                    );
                    println!("{}", first);
                    let _ = output.flush();
                } else {
                    deferred = Some(
                        json!({"jsonrpc":"2.0","id":id,"result":{"thread":{"id":"thread-first"}}}),
                    );
                }
                continue;
            }
            if method == "turn/start" {
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"thread-test","turn":{"id":"turn-test"}}})
                );
                if scenario == "notification-order" {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"item/started","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"first"}}})
                    );
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"second"}}})
                    );
                }
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"item-test","type":"agentMessage","text":"hello"}}})
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
            if scenario == "duplicate-response" && method == "thread/start" {
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","id":id,"result":{"thread":{"id":"duplicate-ignored"}}})
                );
            }
            let _ = output.flush();
        }
    }
}
