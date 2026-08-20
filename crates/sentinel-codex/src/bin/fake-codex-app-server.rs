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
            if method != "initialize" && method != "account/rateLimits/read" {
                non_initialize_requests += 1;
            }
            if scenario == "exit-during-request" && method != "initialize" {
                std::process::exit(17);
            }
            if scenario == "timeout-one" && method != "initialize" && non_initialize_requests == 1 {
                continue;
            }
            if method == "thread/start"
                && matches!(
                    scenario.as_str(),
                    "split-frame"
                        | "multiple-frames"
                        | "malformed-followed-valid"
                        | "oversized-followed-valid"
                )
            {
                let response =
                    json!({"jsonrpc":"2.0","id":id,"result":{"thread":{"id":"thread-test"}}});
                match scenario.as_str() {
                    "split-frame" => {
                        let encoded = response.to_string();
                        let split = encoded.len() / 2;
                        output.write_all(encoded[..split].as_bytes()).unwrap();
                        output.flush().unwrap();
                        output.write_all(encoded[split..].as_bytes()).unwrap();
                        output.write_all(b"\n").unwrap();
                    }
                    "multiple-frames" => {
                        output
                            .write_all(
                                format!(
                                    "{}\n{}\n",
                                    json!({"jsonrpc":"2.0","method":"thread/updated","params":{"threadId":"thread-test","id":"coalesced-notification"}}),
                                    response
                                )
                                .as_bytes(),
                            )
                            .unwrap();
                    }
                    "malformed-followed-valid" => {
                        output
                            .write_all(format!("not-json\n{}\n", response).as_bytes())
                            .unwrap();
                    }
                    "oversized-followed-valid" => {
                        output.write_all(&vec![b'x'; 16 * 1024 + 32]).unwrap();
                        output
                            .write_all(format!("\n{}\n", response).as_bytes())
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                output.flush().unwrap();
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
                if scenario == "implementation-policy" {
                    let valid = value.get("params").is_some_and(|params| {
                        params.get("approvalPolicy").and_then(Value::as_str) == Some("never")
                            && params
                                .get("sandboxPolicy")
                                .and_then(|policy| policy.get("type"))
                                .and_then(Value::as_str)
                                == Some("workspaceWrite")
                    });
                    if !valid {
                        println!(
                            "{}",
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"missing implementation policy"}})
                        );
                        let _ = output.flush();
                        continue;
                    }
                }
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
                if scenario == "duplicate-notification" {
                    let notification = json!({"jsonrpc":"2.0","method":"item/started","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"duplicate-item"}}});
                    println!("{notification}");
                    println!("{notification}");
                }
                if scenario == "out-of-order-item" {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"out-of-order"}}})
                    );
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"item/started","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"out-of-order"}}})
                    );
                }
                if scenario == "late-notification" {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-test","turn":{"id":"turn-test"}}})
                    );
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"late-item"}}})
                    );
                }
                if scenario == "bounded-diagnostic" {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"item/updated","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"large-item","text":"x".repeat(8 * 1024)}}})
                    );
                }
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-test","turnId":"turn-test","item":{"id":"item-test","type":"agentMessage","text":"hello"}}})
                );
                if scenario != "late-notification" {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-test","turn":{"id":"turn-test"}}})
                    );
                }
            }
            if scenario == "rate-limit-update" && method == "thread/start" {
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":77,"windowDurationMins":300,"resetsAt":"2026-08-06T13:00:00Z"}}}})
                );
            }
            if scenario == "unsupported-notification" && method == "thread/start" {
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","method":"future/unsupported","params":{"threadId":"thread-test","providerId":"future-1"}})
                );
            }
            let result = match method {
                "initialize"
                    if scenario == "unsupported-version" || scenario == "newer-version" =>
                {
                    json!({"protocolVersion":"999"})
                }
                "initialize" if scenario == "older-version" => json!({"protocolVersion":"0"}),
                "initialize" if scenario == "missing-version" => json!({}),
                "initialize" if scenario == "protocol-v2" => json!({"protocolVersion":"2"}),
                "initialize" => json!({"protocolVersion":"1"}),
                "account/rateLimits/read" => {
                    json!({"rateLimits":{"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":"2026-08-06T12:00:00Z"}}})
                }
                "thread/start" | "thread/resume" if scenario == "unsupported-payload" => {
                    json!({"thread":{}})
                }
                "thread/start" | "thread/resume" => json!({"thread":{"id":"thread-test"}}),
                "thread/read" if scenario == "missing-thread" => {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32001,"message":"thread not found"}})
                    );
                    let _ = output.flush();
                    continue;
                }
                "thread/read" if scenario == "inactive-thread" => {
                    json!({"thread":{"id":"thread-test","status":{"type":"idle"},"turns":[{"id":"turn-test","status":"completed"}]}})
                }
                "thread/read" => {
                    json!({"thread":{"id":"thread-test","status":{"type":"active"},"turns":[{"id":"turn-test","status":"inProgress"}]}})
                }
                "thread/list" if scenario == "missing-thread" => json!({"data":[]}),
                "thread/list" => json!({"data":[{"id":"thread-test"}]}),
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
