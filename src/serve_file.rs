use std::path::Path;

use futures::TryStreamExt;
use http_body_util::{Full, StreamBody};
use hyper::{body::Frame, Response, StatusCode};
use tokio_util::io::ReaderStream;

use crate::proxy_handler::{BoxedFrameStream, ProxyHandlerBody};

pub async fn serve_file() -> Response<ProxyHandlerBody> {
    println!("Serving file");

    let base_dir = "./";
    let file_path = Path::new(base_dir).join("file.html");

    match tokio::fs::File::open(&file_path).await {
        Ok(file) => {
            let mime_type = mime_guess::from_path(&file_path)
                .first_or_octet_stream()
                .to_string();

            let reader_stream = ReaderStream::new(file)
                .map_ok(Frame::data)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
            let boxed_stream: BoxedFrameStream = Box::pin(reader_stream);

            let body = ProxyHandlerBody::StreamBody(StreamBody::new(boxed_stream));

            let res = Response::builder()
                .status(200)
                .header("Content-Type", mime_type)
                .body(body)
                .unwrap();

            return res;
        }
        Err(err) => {
            println!("Error: {}", err);
            let res = Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(ProxyHandlerBody::Full(Full::from("File not found")))
                .unwrap();
            return res;
        }
    };
}
