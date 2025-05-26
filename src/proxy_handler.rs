use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use http_body_util::Full;
use hyper::{
    body::{Bytes, Frame, Incoming},
    Request, Response,
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use tokio::time::timeout;

use crate::{config::ServerParams, error, utils};

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

    // Redirect to HTTPS if the server has TLS configuration.
    if let Some(dom) = params
        .auto_tls
        .as_ref()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|x| x.starts_with(&domain.to_string()))
    {
        return Ok(Response::builder()
            .status(301)
            .header("Location", format!("https://{}{}", dom, path))
            .body(ProxyHandlerBody::Empty)
            .unwrap());
    }

    // Check for redirections.
    if let Some(redirection) = params
        .redirections
        .get(format!("{}{}", domain, utils::remove_last_slash(path)).as_str())
    {
        return Ok(Response::builder()
            .status(302)
            .header("Location", redirection)
            .body(ProxyHandlerBody::Empty)
            .unwrap());
    }

    // Get the domain (and remove port) from host.
    let domain_copy = domain.to_string();
    let uri_string = if let Some(target) = params
        .targets
        .get(format!("{}{}", domain, utils::remove_last_slash(path)).as_str())
    {
        target
    } else {
        let target = params.targets.get(domain).unwrap();
        &format!("{}{}", target, path)
    };
    // let uri_string = format!("{}{}", target, path);
    let client: Client<_, Incoming> = Client::builder(TokioExecutor::new()).build_http();
    let (parts, body) = req.into_parts();

    println!("{} -> {}", domain_copy, uri_string);

    // Request the targeted server.
    let mut new_req: Request<Incoming> = Request::builder()
        .method(parts.method)
        .uri(uri_string)
        .body(body)
        .expect("request builder");

    *new_req.headers_mut() = parts.headers;

    let future = client.request(new_req);

    // Embeding the future in a timeout.
    // If the request is too long, return a 504 error.
    let pending_future = timeout(Duration::from_secs(params.proxy_timeout), future).await;

    let response: Result<Response<Incoming>, hyper_util::client::legacy::Error>;
    match pending_future {
        // Use the response from the future.
        Ok(res) => {
            response = res;
        }
        // Get the error from the timeout and return a 504 error.
        Err(err) => {
            println!("Error: {:?}", err);
            return Ok(error::gateway_timeout());
        }
    };

    // Return the response from the request.
    match response {
        // If the request succeeded, return the response.
        // It's the data from the targeted server.
        Ok(res) => {
            let res = res.map(ProxyHandlerBody::Incoming);
            return Ok(res);
        }
        // If the request failed, return a 502 error.
        Err(err) => {
            println!("Error: {:?}", err);
            return Ok(error::bad_gateway());
        }
    };
}
