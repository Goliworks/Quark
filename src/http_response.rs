// Some http errors.
use http_body_util::Full;
use hyper::{Response, StatusCode};

use crate::proxy_handler::ProxyHandlerBody;

pub fn not_found() -> Response<ProxyHandlerBody> {
    error_builder(StatusCode::NOT_FOUND)
}

pub fn internal_server_error() -> Response<ProxyHandlerBody> {
    error_builder(StatusCode::INTERNAL_SERVER_ERROR)
}

pub fn bad_gateway() -> Response<ProxyHandlerBody> {
    error_builder(StatusCode::BAD_GATEWAY)
}

pub fn gateway_timeout() -> Response<ProxyHandlerBody> {
    error_builder(StatusCode::GATEWAY_TIMEOUT)
}

fn error_builder(status: StatusCode) -> Response<ProxyHandlerBody> {
    let version = format!("{} v.{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    let code = status.as_u16();
    let msg = status.canonical_reason().unwrap();
    let text = format!(
        "<html>\
        <head><title>{code} {msg}</title></head>\
        <body style='text-align: center; margin-top: 50px;\
        font-family: sans-serif;'>\
        <h1> Error {code}</h1>\
        <h4>{msg}</h4>\
        <hr/>
        <p>{version}</p>\
        </body>\
        </html>",
    );

    Response::builder()
        .status(status)
        .body(ProxyHandlerBody::Full(Full::from(text)))
        .unwrap()
}
