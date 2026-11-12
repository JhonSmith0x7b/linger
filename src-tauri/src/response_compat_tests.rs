//! Offline replays derived from the user's 2026-11-12 compatibility evidence.
//! All original inputs were artificial. Fixtures retain only protocol events;
//! request/account metadata is removed, item IDs are normalized per case, and
//! encrypted reasoning is replaced with an opaque placeholder. No live API calls.
use crate::api::Api;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

fn replay(events: &[Value]) -> (String, thread::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "replay request never arrived");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("replay accept: {error}"),
            }
        };
        // Accepted sockets can inherit the listener's nonblocking mode on
        // macOS. Only accept is polled; request I/O must block with a timeout.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        let (header_end, length) = loop {
            let mut bytes = [0; 4096];
            let count = stream.read(&mut bytes).unwrap();
            assert!(count > 0, "request ended before headers");
            request.extend_from_slice(&bytes[..count]);
            if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                assert!(headers.starts_with("POST /v1/responses "));
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (end + 4, length);
            }
            assert!(request.len() < 64 * 1024, "oversized test headers");
        };
        while request.len() < header_end + length {
            let mut bytes = [0; 4096];
            let count = stream.read(&mut bytes).unwrap();
            assert!(count > 0, "request ended before body");
            request.extend_from_slice(&bytes[..count]);
        }
        let request: Value =
            serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        request
    });
    (url, server)
}

fn fixtures() -> Vec<Value> {
    serde_json::from_str(include_str!("test-fixtures/responses-proxy.json")).unwrap()
}

fn expected_output(events: &[Value]) -> Value {
    let final_output = &events.last().unwrap()["response"]["output"];
    if !final_output.as_array().unwrap().is_empty() {
        return final_output.clone();
    }
    let items: BTreeMap<_, _> = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| {
            (
                event["output_index"].as_u64().unwrap(),
                event["item"].clone(),
            )
        })
        .collect();
    Value::Array(items.into_values().collect())
}

#[tokio::test]
async fn replays_four_model_evidence_preserving_complete_items() {
    let cases = fixtures();
    assert_eq!(cases.len(), 24);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let events = case["events"].as_array().unwrap();
        let expected = expected_output(events);
        assert!(!expected.as_array().unwrap().is_empty(), "{name}");
        let (url, server) = replay(events);
        let mut deltas = String::new();
        let result = Api::new(&url, "synthetic-key")
            .unwrap()
            .response_stream(
                "synthetic-model",
                "test",
                json!([{ "role": "user", "content": "test" }]),
                json!([]),
                4096,
                |delta| deltas.push_str(delta),
            )
            .await;
        let request = server.join().unwrap();
        assert_eq!(request["stream"], true, "{name}");
        assert_eq!(request["store"], false, "{name}");
        let result = result.unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            result["output"], expected,
            "{name}: full items/annotations/arguments/reasoning must survive"
        );
        assert_eq!(
            result["usage"],
            events.last().unwrap()["response"]["usage"],
            "{name}"
        );
        let expected_deltas = events
            .iter()
            .filter(|event| event["type"] == "response.output_text.delta")
            .filter_map(|event| event["delta"].as_str())
            .collect::<String>();
        assert_eq!(
            deltas, expected_deltas,
            "{name}: no duplicate visible deltas"
        );
    }
}

#[tokio::test]
async fn buffered_call_uses_stream_and_returns_complete_json_text() {
    let cases = fixtures();
    let case = cases
        .iter()
        .find(|case| case["name"] == "gpt-6-astra/07_json_1")
        .unwrap();
    let events = case["events"].as_array().unwrap();
    let (url, server) = replay(events);
    let result = Api::new(&url, "synthetic-key")
        .unwrap()
        .response(
            "synthetic-model",
            "return JSON",
            json!([{ "role": "user", "content": "test" }]),
            json!([]),
            4096,
        )
        .await;
    let request = server.join().unwrap();
    assert_eq!(request["stream"], true);
    assert_eq!(request["store"], false);
    let result = result.unwrap();
    assert_eq!(result["output"], expected_output(events));
    let text = result["output"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<String>();
    assert!(serde_json::from_str::<Value>(&text).unwrap().is_object());
}
