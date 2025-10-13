use std::{
    convert::Infallible,
    net::SocketAddr,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{Context, Poll},
};

use http_body_util::{Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    header::{HeaderName, HeaderValue},
    service::service_fn,
    HeaderMap, Request, Response,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use nix::unistd::getuid;
use tokio::net::TcpListener;

use crate::config::ConfigHeadersActions;

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
                    eprintln!("Error: {err}");
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

pub trait HasMutableHeaders {
    fn headers_mut(&mut self) -> &mut HeaderMap;
}

impl<T> HasMutableHeaders for Request<T> {
    fn headers_mut(&mut self) -> &mut HeaderMap {
        self.headers_mut()
    }
}

impl<T> HasMutableHeaders for Response<T> {
    fn headers_mut(&mut self) -> &mut HeaderMap {
        self.headers_mut()
    }
}

pub fn custom_headers<T: HasMutableHeaders>(req: &mut T, headers_actions: &ConfigHeadersActions) {
    if let Some(h) = &headers_actions.set {
        for (k, v) in h {
            req.headers_mut().insert(
                HeaderName::from_str(k).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
    }

    if let Some(h) = &headers_actions.del {
        for k in h {
            req.headers_mut().remove(HeaderName::from_str(k).unwrap());
        }
    }
}

pub async fn welcome_server(http: Arc<Builder<TokioExecutor>>) {
    let port: u16 = if getuid().is_root() { 80 } else { 8080 };
    let socket_addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = TcpListener::bind(socket_addr).await.unwrap();

    loop {
        let http = Arc::clone(&http);
        let (stream, _) = listener.accept().await.unwrap();
        tokio::task::spawn(async move {
            if let Err(err) = http
                .serve_connection(TokioIo::new(stream), service_fn(welcome_server_msg))
                .await
            {
                tracing::error!("failed to serve connection: {err:#}");
            }
        });
    }
}

async fn welcome_server_msg(_: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let version = format!("{} v.{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    let msg = format!(
        "
        <html>\
            <head><title>Quark is ready!</title></head>\
            <body style='text-align:center; margin-top: 50px;\
            font-family: sans-serif;'>\
                <hr/>
                <h1>Quark is ready!</h1>\
                <p>The server has been successfully installed and started.</p>\
                <p>A configuration file is already in place, but it's currently empty.</p>\
                <p>Edit the configuration to define how the server should behave.</p>\
                <p>Once configured, Quark will be ready to serve your content.</p>\
                <br/>
                <hr/>
                <p>{version}</p>\
            </body>
        </html>"
    );
    Ok(Response::new(Full::from(msg)))
}
