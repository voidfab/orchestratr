//! Pins the `agent.start` wire shape against a stub herdr socket.
//!
//! orcr spent a release sending the herdr protocol-16 `agent.start` (`argv` + `cwd`/`env`/
//! `focus`/`split`/`tab_id`/`workspace_id`) to a herdr 0.8.0 that wanted protocol 20
//! (`kind` + `pane_id` + `args`). The handshake is a `>=` floor, so nothing caught it until
//! every spawn died on the socket with `missing field 'kind'`. These tests run without herdr
//! and assert the bytes orcr actually puts on the wire.

use orchestratr::driver::{AgentStartParams, HerdrDriver, MIN_HERDR_PROTOCOL};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;

/// A stub herdr that answers `ping`, `tab.create` and `agent.start`, and reports every
/// request it saw back over a channel. herdr closes after one request per connection.
fn spawn_stub(
    path: std::path::PathBuf,
) -> (
    thread::JoinHandle<()>,
    mpsc::Receiver<(String, serde_json::Value)>,
) {
    let listener = UnixListener::bind(&path).expect("bind stub socket");
    let (tx, rx) = mpsc::channel();
    let h = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                break;
            }
            let req: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => break,
            };
            let id = req["id"].as_str().unwrap_or("x").to_string();
            let method = req["method"].as_str().unwrap_or_default().to_string();
            let params = req["params"].clone();

            let result = match method.as_str() {
                "ping" => serde_json::json!({
                    "type": "pong", "version": "0.8.0", "protocol": MIN_HERDR_PROTOCOL
                }),
                "tab.create" => serde_json::json!({
                    "type": "tab_created",
                    "tab": {
                        "tab_id": "w1:t2", "workspace_id": "w1", "number": 2,
                        "label": "", "focused": false, "pane_count": 1,
                        "agent_status": "unknown"
                    },
                    "root_pane": {
                        "pane_id": "w1:p7", "terminal_id": "term-7", "workspace_id": "w1",
                        "tab_id": "w1:t2", "focused": false, "agent_status": "unknown",
                        "revision": 1
                    }
                }),
                "agent.start" => serde_json::json!({
                    "type": "agent_started",
                    "argv": ["claude"],
                    "agent": {
                        "terminal_id": "term-7", "agent_status": "unknown",
                        "workspace_id": "w1", "tab_id": "w1:t2", "pane_id": "w1:p7",
                        "focused": false, "revision": 2
                    }
                }),
                _ => serde_json::json!({ "type": "ok" }),
            };
            let mut out =
                serde_json::to_vec(&serde_json::json!({ "id": id, "result": result })).unwrap();
            out.push(b'\n');
            let _ = stream.write_all(&out);
            let _ = stream.flush();
            let done = method == "agent.start";
            let _ = tx.send((method, params));
            if done {
                break;
            }
        }
    });
    (h, rx)
}

fn params() -> AgentStartParams {
    AgentStartParams {
        name: "horos/r1/iteration".into(),
        argv: vec![
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            "--model".into(),
            "opus".into(),
        ],
        cwd: Some("/Users/dk/ghq/github.com/voidfab/horos".into()),
        env: [("ORCR_AGENT".to_string(), "1".to_string())]
            .into_iter()
            .collect(),
        focus: false,
        workspace_id: Some("w1".into()),
    }
}

#[test]
fn agent_start_sends_the_protocol_20_shape() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("stub.sock");
    let (h, rx) = spawn_stub(sock.clone());

    let driver = HerdrDriver::connect(&sock).expect("handshake");
    let info = driver.agent_start(&params()).expect("agent.start");
    assert_eq!(info.pane_id, "w1:p7");
    assert_eq!(info.terminal_id, "term-7");

    let seen: Vec<_> = rx.iter().collect();
    let methods: Vec<&str> = seen.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(methods, ["ping", "tab.create", "agent.start"]);

    // cwd + env moved onto tab.create: agent.start no longer accepts either.
    let tab = &seen[1].1;
    assert_eq!(tab["workspace_id"], "w1");
    assert_eq!(tab["cwd"], "/Users/dk/ghq/github.com/voidfab/horos");
    assert_eq!(tab["env"]["ORCR_AGENT"], "1");
    assert_eq!(tab["focus"], false);

    // The shape that broke: `kind` + `pane_id` + `args`, and no protocol-16 leftovers.
    let start = &seen[2].1;
    assert_eq!(start["name"], "horos/r1/iteration");
    assert_eq!(start["kind"], "claude");
    assert_eq!(start["pane_id"], "w1:p7");
    assert_eq!(
        start["args"],
        serde_json::json!(["--dangerously-skip-permissions", "--model", "opus"])
    );
    for gone in ["argv", "cwd", "env", "focus", "split", "tab_id", "workspace_id"] {
        assert!(
            start.get(gone).is_none(),
            "agent.start must not carry `{gone}` under protocol 20"
        );
    }

    h.join().unwrap();
}

#[test]
fn agent_start_closes_the_fresh_pane_when_the_start_fails() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("stub_fail.sock");
    let listener = UnixListener::bind(&sock).expect("bind");
    let (tx, rx) = mpsc::channel();
    let h = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                break;
            }
            let req: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => break,
            };
            let id = req["id"].as_str().unwrap_or("x").to_string();
            let method = req["method"].as_str().unwrap_or_default().to_string();
            let body = match method.as_str() {
                "ping" => serde_json::json!({ "id": id, "result": {
                    "type": "pong", "version": "0.8.0", "protocol": MIN_HERDR_PROTOCOL }}),
                "tab.create" => serde_json::json!({ "id": id, "result": {
                    "type": "tab_created",
                    "tab": { "tab_id": "w1:t2", "workspace_id": "w1", "number": 2,
                             "label": "", "focused": false, "pane_count": 1,
                             "agent_status": "unknown" },
                    "root_pane": { "pane_id": "w1:p7", "terminal_id": "term-7",
                                   "workspace_id": "w1", "tab_id": "w1:t2", "focused": false,
                                   "agent_status": "unknown", "revision": 1 }}}),
                "agent.start" => serde_json::json!({ "id": id, "error": {
                    "code": "agent_pane_busy", "message": "pane w1:p7 is busy" }}),
                _ => serde_json::json!({ "id": id, "result": { "type": "ok" }}),
            };
            let mut out = serde_json::to_vec(&body).unwrap();
            out.push(b'\n');
            let _ = stream.write_all(&out);
            let _ = stream.flush();
            let done = method == "pane.close";
            let _ = tx.send(method);
            if done {
                break;
            }
        }
    });

    let driver = HerdrDriver::connect(&sock).expect("handshake");
    let err = driver.agent_start(&params()).expect_err("start must fail");
    assert_eq!(err.details["herdr_code"], "agent_pane_busy");

    let methods: Vec<String> = rx.iter().collect();
    assert_eq!(methods, ["ping", "tab.create", "agent.start", "pane.close"]);

    h.join().unwrap();
}
