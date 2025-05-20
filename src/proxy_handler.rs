use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use http_body_util::Full;
use hyper::{
    body::{Bytes, Frame, Incoming},
    Request, Response,
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};

use crate::config::ServerParams;

pub enum ProxyHandlerBody {
    Incoming(hyper::body::Incoming),
    Full(Full<Bytes>),
    Empty,
}

impl hyper::body::Body for ProxyHandlerBody {
    type Data = hyper::body::Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut *self.get_mut() {
            Self::Incoming(incoming) => Pin::new(incoming).poll_frame(cx),
            Self::Full(full) => match Pin::new(full).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(_err))) => {
                    unreachable!("Full<Bytes> cannot error (Infallible)")
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            Self::Empty => Poll::Ready(None),
        }
    }
}

pub async fn proxy_handler(
    req: Request<Incoming>,
    params: Arc<ServerParams>,
) -> Result<Response<ProxyHandlerBody>, hyper_util::client::legacy::Error> {
    // Get the domain.
    // Use authority for HTTP/2
    let domain = if req.uri().authority().is_some() {
        req.uri().authority().unwrap().host()
    } else {
        req.headers()["host"]
            .to_str()
            .unwrap()
            .split(':')
            .next()
            .unwrap()
    };
    // Get the path from the request.
    let path = req.uri().path_and_query().unwrap().path();

    // return Ok(Response::builder()
    //     .status(200)
    //     .body(ProxyHandlerBody::Full(Full::from("aaaaaaa")))
    //     .unwrap());

    // Redirect to HTTPS if the server has TLS configuration.
    if let Some(dom) = params
        .auto_tls
        .as_ref()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|x| x.starts_with(&domain.to_string()))
    {
        return Ok(Response::builder()
            .status(302)
            .header("Location", format!("https://{}{}", dom, path))
            .body(ProxyHandlerBody::Empty)
            .unwrap());
    }

    // Get the domain (and remove port) from host.
    let target = params.targets.get(domain).unwrap();
    let uri_string = format!("http://{}{}", target, path);
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let (parts, _body) = req.into_parts();

    // Request the targeted server.
    let mut new_req: Request<Full<Bytes>> = Request::builder()
        .method(parts.method)
        .uri(uri_string)
        .body(Full::from(""))
        .expect("request builder");

    *new_req.headers_mut() = parts.headers;

    let future = client.request(new_req);

    future.await.and_then(|resp| {
        let resp = resp.map(ProxyHandlerBody::Incoming);
        Ok(resp)
    })
}
