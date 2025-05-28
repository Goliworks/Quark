use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use http_body_util::{Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    Request, Response, StatusCode,
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use tokio::time::timeout;

use crate::{config::ServerParams, http_response, serve_file::serve_file, utils};

pub type BoxedFrameStream =
    Pin<Box<dyn futures::Stream<Item = Result<Frame<Bytes>, std::io::Error>> + Send + 'static>>;

pub enum ProxyHandlerBody {
    Incoming(Incoming),
    Full(Full<Bytes>),
    StreamBody(StreamBody<BoxedFrameStream>),
    Empty,
}

impl hyper::body::Body for ProxyHandlerBody {
    type Data = hyper::body::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut *self.get_mut() {
            Self::Incoming(incoming) => match Pin::new(incoming).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(err))) => {
                    println!("Error: {}", err);
                    Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        err,
                    ))))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            Self::Full(full) => match Pin::new(full).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(_err))) => {
                    unreachable!("Full<Bytes> cannot error (Infallible)")
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            Self::StreamBody(stream_body) => Pin::new(stream_body).poll_frame(cx),
            Self::Empty => Poll::Ready(None),
        }
    }
}

pub async fn proxy_handler(
    req: Request<Incoming>,
    params: Arc<ServerParams>,
) -> Result<Response<ProxyHandlerBody>, hyper::Error> {
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
            .status(StatusCode::PERMANENT_REDIRECT)
            .header("Location", format!("https://{}{}", dom, path))
            .body(ProxyHandlerBody::Empty)
            .unwrap());
    }

    // Check for redirections.

    let match_url = format!("{}{}", domain, utils::remove_last_slash(path));

    match params.redirections.get(match_url.as_str()) {
        // First, check for a strict match.
        Some(redirection) => {
            return Ok(Response::builder()
                .status(redirection.code)
                .header("Location", redirection.location.clone())
                .body(ProxyHandlerBody::Empty)
                .unwrap());
        }
        // If no strict match, check for a match with the path.
        None => {
            let mut uri_path: Option<String> = None;
            let mut red_code: Option<u16> = None;
            for (url, target) in params.redirections.iter().rev() {
                if !target.strict_uri && match_url.as_str().starts_with(url.as_str()) {
                    let new_path = match_url.strip_prefix(url);
                    uri_path = Some(format!(
                        "{}{}",
                        utils::remove_last_slash(&target.location),
                        new_path.unwrap()
                    ));
                    red_code = Some(target.code);
                    break;
                }
            }

            if let Some(uri) = uri_path {
                return Ok(Response::builder()
                    .status(red_code.unwrap())
                    .header("Location", uri)
                    .body(ProxyHandlerBody::Empty)
                    .unwrap());
            }
        }
    }

    // Get the domain (and remove port) from host.

    let uri_string: Result<(String, bool), _> = match params.targets.get(match_url.as_str()) {
        // First, check for a strict match.
        Some(target) => Ok((target.location.clone(), target.serve_files)),
        // If no strict match, check for a match with the path.
        None => {
            let mut uri_path: Option<String> = None;
            let mut serve_files: Option<bool> = None;
            for (url, target) in params.targets.iter().rev() {
                if !target.strict_uri && match_url.as_str().starts_with(url.as_str()) {
                    let new_path = match_url.strip_prefix(url);
                    uri_path = Some(format!(
                        "{}{}",
                        utils::remove_last_slash(&target.location),
                        new_path.unwrap()
                    ));
                    serve_files = Some(target.serve_files);
                    break;
                }
            }

            match uri_path {
                Some(uri) => Ok((uri, serve_files.unwrap())),
                None => Err(()),
            }
        }
    };

    // Build the client.
    let client: Client<_, Incoming> = Client::builder(TokioExecutor::new()).build_http();
    // Extract parts and body from the request.
    let (parts, body) = req.into_parts();

    // Request the targeted server.
    let mut new_req: Request<Incoming> = match uri_string {
        Ok((uri, serve_files)) => {
            if !serve_files {
                // Build the reverse proxy request
                Request::builder()
                    .method(parts.method)
                    .uri(uri)
                    .body(body)
                    .expect("request builder")
            } else {
                // Serve files. Return directly the response.
                let sf = serve_file(&uri).await;
                return Ok(sf);
            }
        }
        Err(_) => return Ok(http_response::internal_server_error()),
    };

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
            return Ok(http_response::gateway_timeout());
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
            return Ok(http_response::bad_gateway());
        }
    };
}
