use std::path::{Component, Path, PathBuf};

use futures::TryStreamExt;
use http_body_util::StreamBody;
use hyper::{body::Frame, Response};
use tokio_util::io::ReaderStream;

use crate::http_response;

use super::server_utils::{BoxedFrameStream, ProxyHandlerBody};

// Simple file server.
pub async fn serve_file(path: &str) -> Response<ProxyHandlerBody> {
    tracing::info!("Serve file : {}", path);

    let mut file_path = sanitize_path(path);

    if file_path.is_dir() {
        // Try to open index.html.
        file_path.push("index.html");
        return match open_file(&file_path).await {
            Ok(resp) => resp,
            // Default forbidden response if the path is a dir.
            Err(_) => http_response::forbidden(),
        };
    }

    match open_file(&file_path).await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!("Serving file Error: {}", err);
            http_response::not_found()
        }
    }
}

async fn open_file(file_path: &PathBuf) -> Result<Response<ProxyHandlerBody>, std::io::Error> {
    match tokio::fs::File::open(file_path).await {
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

            Ok(res)
        }
        Err(err) => Err(err),
    }
}

fn sanitize_path(path: &str) -> PathBuf {
    let mut clean_path = PathBuf::new();

    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => clean_path.push(part),
            Component::ParentDir => continue,
            Component::CurDir => continue,
            Component::RootDir => clean_path.push("/"),
            Component::Prefix(_) => continue,
        }
    }

    clean_path
}
