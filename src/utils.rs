use std::{
    pin::Pin,
    task::{Context, Poll},
};

use http_body_util::{Full, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};

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

pub fn remove_last_slash(path: &str) -> &str {
    if path.ends_with("/") {
        &path[..path.len() - 1]
    } else {
        path
    }
}

pub fn format_ip(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some() => {
            v6.to_ipv4().unwrap().to_string()
        }
        _ => ip.to_string(),
    }
}
