// Simple HTTP Server Example
// Demonstrates WASI HTTP component with basic request/response handling

wit_bindgen::generate!({
    world: "hello-http-server",
    path: "wit",
    generate_all,
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{
    Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

struct HelloHttpServer;

impl Guest for HelloHttpServer {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        eprintln!("[INFO] Received HTTP request");

        // Extract request details
        let method = request.method();
        let path = request.path_with_query().unwrap_or_else(|| "/".to_string());

        eprintln!("[DEBUG] Method: {:?}, Path: {}", method, path);

        // Route the request
        let (status, body, content_type) = route_request(&method, &path);

        eprintln!("[METRIC] response_status: {}", status);
        eprintln!("[METRIC] response_bytes: {}", body.len());

        // Create response headers
        let response_headers = Fields::new();
        let _ = response_headers.set(
            &"content-type".to_string(),
            &[content_type.as_bytes().to_vec()],
        );
        let _ = response_headers.set(
            &"content-length".to_string(),
            &[body.len().to_string().as_bytes().to_vec()],
        );

        // Create and send response
        let response = OutgoingResponse::new(response_headers);
        response.set_status_code(status).ok();

        let response_body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        // Write body
        let output_stream = response_body.write().unwrap();
        output_stream.blocking_write_and_flush(&body).ok();
        OutgoingBody::finish(response_body, None).ok();

        eprintln!("[INFO] Request handled successfully");
    }
}

export!(HelloHttpServer);

/// Simple request router
fn route_request(method: &Method, path: &str) -> (u16, Vec<u8>, String) {
    match (method, path) {
        (Method::Get, "/") => {
            eprintln!("[DEBUG] Serving root endpoint");
            let body = "Hello, World!\nWelcome to the WebAssembly Compositional System HTTP Example!";
            (200, body.as_bytes().to_vec(), "text/plain".to_string())
        }

        (Method::Get, "/hello") => {
            eprintln!("[DEBUG] Serving /hello endpoint");
            let body = r#"{"message": "Hello from WebAssembly!", "version": "0.1.0"}"#;
            (200, body.as_bytes().to_vec(), "application/json".to_string())
        }

        (Method::Get, "/health") => {
            eprintln!("[DEBUG] Health check");
            let body = r#"{"status": "healthy", "timestamp": 1234567890}"#;
            (200, body.as_bytes().to_vec(), "application/json".to_string())
        }

        _ => {
            eprintln!("[DEBUG] Route not found: {}", path);
            let body = r#"{"error": "Not Found", "path": "unknown"}"#;
            (404, body.as_bytes().to_vec(), "application/json".to_string())
        }
    }
}
