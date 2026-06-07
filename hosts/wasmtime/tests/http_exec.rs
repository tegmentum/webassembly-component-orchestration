//! HTTP execution test (feature-gated). Drives the `hello-http` wasi:http
//! component through `handle`, asserting it routes `GET /`.
//!
//! Build the component first: examples/hello-http/build.sh (or
//! `cargo build --release --target wasm32-wasip2` in that dir). Skips
//! gracefully if the artifact is absent.
#![cfg(feature = "http-server")]

use compose_host_wasmtime::{http, HttpRequest};
use std::path::PathBuf;

fn hello_http() -> Option<Vec<u8>> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/hello-http/target/wasm32-wasip2/release/hello_http.wasm"),
    )
    .ok()
}

#[test]
fn handle_http_routes_get_root() {
    let Some(bytes) = hello_http() else {
        eprintln!("skipping: build examples/hello-http first");
        return;
    };
    let req = HttpRequest {
        method: "GET".to_string(),
        path: "/".to_string(),
        headers: vec![],
        body: vec![],
    };
    let resp = http::handle(&bytes, &req).expect("handle GET /");
    assert_eq!(resp.status, 200, "status");
    let body = String::from_utf8_lossy(&resp.body);
    assert!(body.contains("Hello, World!"), "body: {body}");
}
