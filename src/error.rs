// Some http errors.
use http_body_util::Full;
use hyper::Response;

use crate::proxy_handler::ProxyHandlerBody;

pub fn bad_gateway() -> Response<ProxyHandlerBody> {
    error_builder(502, "Bad gateway")
}

pub fn gateway_timeout() -> Response<ProxyHandlerBody> {
    error_builder(504, "Gateway timeout")
}

fn error_builder(err: u16, msg: &str) -> Response<ProxyHandlerBody> {
    let version = format!("{} v.{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    let text = format!(
        "<html>\
        <head><title>{err} {msg}</title></head>\
        <body style='text-align: center; margin-top: 50px;\
        font-family: sans-serif;'>\
        <h1> Error {err}</h1>\
        <h4>{msg}</h4>\
        <hr/>
        <p>{version}</p>\
        </body>\
        </html>",
    );

    Response::builder()
        .status(err)
        .body(ProxyHandlerBody::Full(Full::from(text)))
        .unwrap()
}
