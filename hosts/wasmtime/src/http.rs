//! HTTP execution for `wasi:http/incoming-handler` components.
//!
//! Backs `exec.handle-http` (run one request) and `exec.serve-http` (a
//! long-running server). `wasi:http` is async, so this path uses its own
//! async-enabled engine + a tokio runtime, isolated behind the
//! `http-server` feature from the otherwise-sync exec path.
//!
//! The per-request driver follows the `wasmtime-wasi-http` p2 recipe:
//! build a `ProxyPre`, create per-request `Store` state, hand the guest an
//! incoming request + response-outparam, and await the response.
use compose_core::types::{Error, ErrorCode, HttpRequest, HttpResponse};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use std::sync::Arc;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::p2::bindings::http::types::Scheme;
use wasmtime_wasi_http::p2::bindings::ProxyPre;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::{WasiHttpCtxView, WasiHttpView};
use wasmtime_wasi_http::WasiHttpCtx;

/// Per-request store state: WASI + wasi-http contexts and the resource table.
struct HttpState {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
}

impl WasiView for HttpState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for HttpState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: Default::default(),
        }
    }
}

fn core_err(code: ErrorCode, msg: impl Into<String>) -> Error {
    Error::new(code, msg.into())
}

/// Build an async, component-model engine for the HTTP path.
fn http_engine() -> Result<Engine, Error> {
    // wasmtime 45: async support is always available; no config flag needed.
    let mut config = Config::new();
    config.wasm_component_model(true);
    Engine::new(&config).map_err(|e| core_err(ErrorCode::InternalError, format!("engine: {e:?}")))
}

/// Compile a component and build a `ProxyPre` (links WASI + wasi:http).
fn proxy_pre(engine: &Engine, component_bytes: &[u8]) -> Result<ProxyPre<HttpState>, Error> {
    let component = Component::new(engine, component_bytes)
        .map_err(|e| core_err(ErrorCode::EmitLinkError, format!("load component: {e:?}")))?;
    let mut linker = Linker::<HttpState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)
        .map_err(|e| core_err(ErrorCode::InternalError, format!("add wasi: {e:?}")))?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
        .map_err(|e| core_err(ErrorCode::InternalError, format!("add wasi-http: {e:?}")))?;
    let pre = linker
        .instantiate_pre(&component)
        .map_err(|e| core_err(ErrorCode::EmitLinkError, format!("instantiate_pre: {e:?}")))?;
    ProxyPre::new(pre).map_err(|e| {
        core_err(
            ErrorCode::EmitLinkError,
            format!("component is not a wasi:http proxy: {e:?}"),
        )
    })
}

/// Drive one request through the guest's `incoming-handler`, returning the
/// hyper response. The request body is `HyperOutgoingBody` so it satisfies
/// `new_incoming_request`'s body bound directly.
async fn dispatch(
    pre: ProxyPre<HttpState>,
    request: hyper::Request<HyperOutgoingBody>,
) -> Result<hyper::Response<HyperOutgoingBody>, Error> {
    let mut store = Store::new(
        pre.engine(),
        HttpState {
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
        },
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let req = store
        .data_mut()
        .http()
        .new_incoming_request(Scheme::Http, request)
        .map_err(|e| core_err(ErrorCode::InternalError, format!("incoming request: {e:?}")))?;
    let out = store
        .data_mut()
        .http()
        .new_response_outparam(sender)
        .map_err(|e| {
            core_err(
                ErrorCode::InternalError,
                format!("response outparam: {e:?}"),
            )
        })?;

    let task = tokio::task::spawn(async move {
        let proxy = pre.instantiate_async(&mut store).await?;
        proxy
            .wasi_http_incoming_handler()
            .call_handle(&mut store, req, out)
            .await
    });

    match receiver.await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(core_err(
            ErrorCode::ExecTrap,
            format!("handler error: {e:?}"),
        )),
        Err(_) => {
            // The guest never set a response; surface the task error.
            let msg = match task.await {
                Ok(Ok(())) => "guest did not produce a response".to_string(),
                Ok(Err(e)) => format!("guest trapped: {e:?}"),
                Err(e) => format!("join error: {e:?}"),
            };
            Err(core_err(ErrorCode::ExecTrap, msg))
        }
    }
}

/// Convert a portable `HttpRequest` into a hyper request with a fixed body.
fn to_hyper(req: &HttpRequest) -> Result<hyper::Request<HyperOutgoingBody>, Error> {
    let mut builder = hyper::Request::builder()
        .method(req.method.as_str())
        .uri(&req.path);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    // wasi:http requires an authority (URI authority or Host header). If the
    // caller gave only a path and no Host, default it so the guest sees a
    // well-formed request.
    let has_host = req
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("host"));
    if has_host {
        // already provided
    } else if req.path.starts_with("http://") || req.path.starts_with("https://") {
        // authority is in the URI
    } else {
        builder = builder.header("host", "localhost");
    }
    // A fixed in-memory body; Full is infallible so its error never occurs.
    let body: HyperOutgoingBody = Full::new(Bytes::from(req.body.clone()))
        .map_err(|e| match e {})
        .boxed_unsync();
    builder
        .body(body)
        .map_err(|e| core_err(ErrorCode::InvalidInput, format!("bad request: {e:?}")))
}

/// Convert a hyper response into a portable `HttpResponse` (buffered body).
async fn from_hyper(resp: hyper::Response<HyperOutgoingBody>) -> Result<HttpResponse, Error> {
    let status = resp.status().as_u16() as u32;
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| core_err(ErrorCode::ExecTrap, format!("read response body: {e:?}")))?
        .to_bytes()
        .to_vec();
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// Handle a single HTTP request against a `wasi:http` component (blocking).
pub fn handle(component_bytes: &[u8], request: &HttpRequest) -> Result<HttpResponse, Error> {
    let engine = http_engine()?;
    let pre = proxy_pre(&engine, component_bytes)?;
    let hyper_req = to_hyper(request)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| core_err(ErrorCode::InternalError, format!("runtime: {e:?}")))?;
    rt.block_on(async move {
        let resp = dispatch(pre, hyper_req).await?;
        from_hyper(resp).await
    })
}

/// Serve a `wasi:http` component over HTTP/1.1 on `port` until the process
/// exits (blocking). Each connection is handled on a tokio task.
pub fn serve(component_bytes: &[u8], port: u16) -> Result<(), Error> {
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;

    let engine = http_engine()?;
    let pre = Arc::new(proxy_pre(&engine, component_bytes)?);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| core_err(ErrorCode::InternalError, format!("runtime: {e:?}")))?;

    rt.block_on(async move {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| core_err(ErrorCode::InternalError, format!("bind {addr}: {e:?}")))?;
        tracing::info!("serve-http listening on http://{addr}");

        loop {
            let (client, _peer) = listener
                .accept()
                .await
                .map_err(|e| core_err(ErrorCode::InternalError, format!("accept: {e:?}")))?;
            let pre = pre.clone();
            tokio::task::spawn(async move {
                let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let pre = (*pre).clone();
                    async move {
                        // Adapt the incoming body to HyperOutgoingBody.
                        let (parts, body) = req.into_parts();
                        let body = body.map_err(|e| wasmtime_wasi_http::p2::bindings::http::types::ErrorCode::InternalError(Some(e.to_string()))).boxed_unsync();
                        let req = hyper::Request::from_parts(parts, body);
                        match dispatch(pre, req).await {
                            Ok(resp) => Ok::<_, std::convert::Infallible>(resp),
                            Err(e) => {
                                let body: HyperOutgoingBody =
                                    Full::new(Bytes::from(format!("{}: {}", "exec error", e.message)))
                                        .map_err(|x| match x {})
                                        .boxed_unsync();
                                Ok(hyper::Response::builder().status(500).body(body).unwrap())
                            }
                        }
                    }
                });
                let _ = http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(client), service)
                    .await;
            });
        }
    })
}
