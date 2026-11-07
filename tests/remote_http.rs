use rune_kit::{
    protocol::McpRouter,
    remote::{RemoteConfig, RemoteHttp},
    runtime::PluginInstance,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

struct Request {
    headers: BTreeMap<String, String>,
    body: Value,
}
fn fixture(
    count: usize,
    mut handler: impl FnMut(usize, Request) -> String + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        for index in 0..count {
            let start = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(8) =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    other => panic!("Missing expected remote request {index}: {other:?}"),
                }
            };
            // Accepted sockets inherit nonblocking mode on Windows. Only the
            // accept loop polls; fixture requests use bounded blocking I/O.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut data = Vec::new();
            let end = loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                data.push(byte[0]);
                if data.ends_with(b"\r\n\r\n") {
                    break data.len();
                }
                assert!(data.len() < 65536);
            };
            let head = String::from_utf8(data.clone()).unwrap();
            let headers: BTreeMap<_, _> = head
                .lines()
                .skip(1)
                .filter_map(|line| line.split_once(':'))
                .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
                .collect();
            let len: usize = headers["content-length"].parse().unwrap();
            data.resize(end + len, 0);
            stream.read_exact(&mut data[end..]).unwrap();
            let body = serde_json::from_slice(&data[end..]).unwrap();
            let response = handler(index, Request { headers, body });
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (endpoint, thread)
}
fn http(status: &str, content_type: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
        body.len()
    )
}
fn reply(request: &Request, result: Value) -> String {
    http(
        "200 OK",
        "application/json",
        "",
        &json!({"jsonrpc":"2.0","id":request.body["id"],"result":result}).to_string(),
    )
}
fn client(endpoint: String, protocol: &str) -> RemoteHttp {
    RemoteHttp::new(
        "external".into(),
        RemoteConfig {
            endpoint,
            protocol: protocol.into(),
            token_env: None,
        },
        Some("upstream-secret".into()),
    )
    .unwrap()
}
fn request(router: &mut McpRouter, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
    router
        .handle_jsonrpc(
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        )
        .unwrap()
}

#[test]
fn current_routes_json_and_sse_preserving_metadata_and_resource_identity() {
    let (endpoint, worker) = fixture(7, |index, req| {
        assert_eq!(req.headers["authorization"], "Bearer upstream-secret");
        assert_eq!(req.headers["mcp-protocol-version"], "2026-07-28");
        assert_eq!(req.headers["mcp-method"], req.body["method"]);
        assert!(!req.headers.contains_key("mcp-session-id"));
        assert_eq!(
            req.body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            "2026-07-28"
        );
        match index {
            0 | 1 => reply(
                &req,
                json!({"tools":[{"name":"echo","title":"Echo title","inputSchema":{"type":"object","properties":{"text":{"type":"string","x-mcp-header":"Text"}}},"outputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]}),
            ),
            2 => {
                assert_eq!(req.headers["mcp-name"], "echo");
                assert_eq!(
                    req.headers["mcp-param-text"],
                    rune_kit::headers::encode("snow ☃")
                );
                let event = format!(
                    "data: {}\r\n\r\ndata: {}\r\n\r\n",
                    json!({"jsonrpc":"2.0","method":"notifications/message","params":{}}),
                    json!({"jsonrpc":"2.0","id":req.body["id"],"result":{"content":[],"structuredContent":{"echo":"snow ☃"},"isError":false}})
                );
                http("200 OK", "text/event-stream", "", &event)
            }
            3 => reply(
                &req,
                json!({"resources":[{"uri":"rune://inner/document","name":"doc","title":"Doc title"}]}),
            ),
            4 => {
                assert_eq!(req.body["params"]["uri"], "rune://inner/document");
                reply(
                    &req,
                    json!({"contents":[{"uri":"rune://inner/document","text":"body"}]}),
                )
            }
            5 => reply(
                &req,
                json!({"prompts":[{"name":"ask","title":"Ask title","arguments":[{"name":"topic"}]}]}),
            ),
            6 => reply(&req, json!({"messages":[]})),
            _ => unreachable!(),
        }
    });
    let mut router = McpRouter::new();
    router.register(
        "external".into(),
        PluginInstance::Remote(Box::new(client(endpoint, "2026-07-28"))),
    );
    let tools = request(&mut router, "tools/list", json!({}));
    assert_eq!(tools["result"]["tools"][0]["name"], "external__echo");
    assert_eq!(
        tools["result"]["tools"][0]["outputSchema"],
        json!({"type":"object"})
    );
    assert_eq!(tools["result"]["tools"][0]["title"], "Echo title");
    let call = request(
        &mut router,
        "tools/call",
        json!({"name":"external__echo","arguments":{"text":"snow ☃"}}),
    );
    assert_eq!(call["result"]["structuredContent"]["echo"], "snow ☃");
    let resources = request(&mut router, "resources/list", json!({}));
    assert_eq!(
        resources["result"]["resources"][0]["uri"],
        "rune://external/rune://inner/document"
    );
    assert_eq!(resources["result"]["resources"][0]["title"], "Doc title");
    let read = request(
        &mut router,
        "resources/read",
        json!({"uri":"rune://external/rune://inner/document"}),
    );
    assert_eq!(
        read["result"]["contents"][0]["uri"],
        "rune://external/rune://inner/document"
    );
    let prompts = request(&mut router, "prompts/list", json!({}));
    assert_eq!(prompts["result"]["prompts"][0]["title"], "Ask title");
    assert!(
        request(&mut router, "prompts/get", json!({"name":"external__ask"}))["result"]["messages"]
            .is_array()
    );
    worker.join().unwrap();
}

#[test]
fn legacy_initializes_once_reuses_session_and_does_not_retry_expired_call() {
    let (endpoint, worker) = fixture(4, |index, req| match index {
        0 => {
            assert_eq!(req.body["method"], "initialize");
            assert!(!req.headers.contains_key("mcp-session-id"));
            let body = json!({"jsonrpc":"2.0","id":req.body["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}});
            http(
                "200 OK",
                "application/json",
                "Mcp-Session-Id: fixture-session\r\n",
                &body.to_string(),
            )
        }
        1 => {
            assert_eq!(req.headers["mcp-session-id"], "fixture-session");
            assert_eq!(req.body["method"], "notifications/initialized");
            assert!(req.body.get("id").is_none());
            http("202 Accepted", "application/json", "", "")
        }
        2 => {
            assert_eq!(req.headers["mcp-session-id"], "fixture-session");
            reply(
                &req,
                json!({"tools":[{"name":"write","inputSchema":{"type":"object"}}]}),
            )
        }
        3 => {
            assert_eq!(req.body["method"], "tools/call");
            http("404 Not Found", "application/json", "", "")
        }
        _ => unreachable!(),
    });
    let mut remote = client(endpoint, "2025-11-25");
    remote.list_tools().unwrap();
    assert!(
        remote
            .call_tool("write", json!({}))
            .unwrap_err()
            .to_string()
            .contains("not retried")
    );
    worker.join().unwrap();
}

#[test]
fn rejects_mismatched_ids_invalid_inventory_and_repeated_cursors() {
    for case in 0..3 {
        let (endpoint, worker) = fixture(if case == 2 { 2 } else { 1 }, move |_, req| match case {
            0 => http(
                "200 OK",
                "application/json",
                "",
                &json!({"jsonrpc":"2.0","id":999,"result":{"tools":[]}}).to_string(),
            ),
            1 => reply(&req, json!({"tools":[42]})),
            _ => reply(&req, json!({"tools":[],"nextCursor":"again"})),
        });
        assert!(client(endpoint, "2026-07-28").list_tools().is_err());
        worker.join().unwrap();
    }
}

#[test]
fn cancellation_and_deadline_stop_waiting_for_upstream() {
    for cancel_request in [false, true] {
        let (endpoint, worker) = fixture(1, |_, req| {
            std::thread::sleep(Duration::from_millis(250));
            reply(&req, json!({"tools":[]}))
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let mut remote = client(endpoint, "2026-07-28");
        remote.set_operation_context(
            cancel.clone(),
            Some(Duration::from_millis(if cancel_request {
                2000
            } else {
                80
            })),
        );
        let flag = cancel.clone();
        let trigger = std::thread::spawn(move || {
            if cancel_request {
                std::thread::sleep(Duration::from_millis(80));
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        let started = std::time::Instant::now();
        let error = remote.list_tools().unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(if cancel_request {
            matches!(error, rune_kit::runtime::RuntimeError::Cancelled)
        } else {
            matches!(error, rune_kit::runtime::RuntimeError::Timeout(_))
        });
        trigger.join().unwrap();
        worker.join().unwrap();
    }
}

#[test]
fn redirects_are_not_followed_and_errors_do_not_expose_remote_secrets() {
    let (endpoint, worker) = fixture(1, |_, _| {
        http(
            "307 Temporary Redirect",
            "application/json",
            "Location: http://127.0.0.1:1/secret\r\n",
            "sensitive remote body",
        )
    });
    let error = client(format!("{endpoint}?key=private"), "2026-07-28")
        .list_tools()
        .unwrap_err()
        .to_string();
    assert!(error.contains("307"));
    for secret in [
        "private",
        "upstream-secret",
        "sensitive remote body",
        "127.0.0.1",
    ] {
        assert!(!error.contains(secret));
    }
    worker.join().unwrap();
}

#[test]
fn sse_accepts_bom_multiline_events_and_all_line_endings_across_chunks() {
    for ending in ["\n", "\r", "\r\n"] {
        let (endpoint, worker) = fixture(1, move |_, req| {
            let value = json!({"jsonrpc":"2.0","id":req.body["id"],"result":{"tools":[]}});
            let body = format!(
                "\u{feff}:comment{ending}{}{ending}",
                serde_json::to_string_pretty(&value)
                    .unwrap()
                    .lines()
                    .map(|line| format!("data: {line}{ending}"))
                    .collect::<String>()
            );
            let chunks: String = body
                .chars()
                .map(|ch| format!("{:x}\r\n{ch}\r\n", ch.len_utf8()))
                .collect();
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{chunks}0\r\n\r\n"
            )
        });
        assert!(
            client(endpoint, "2026-07-28")
                .list_tools()
                .unwrap()
                .is_empty()
        );
        worker.join().unwrap();
    }
}
