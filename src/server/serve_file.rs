use std::path::{Component, Path, PathBuf};

use futures::TryStreamExt;
use http_body_util::{Full, StreamBody};
use hyper::{body::Frame, Response, StatusCode};
use time::{
    format_description::{self},
    OffsetDateTime,
};
use tokio_util::io::ReaderStream;

use crate::{http_response, utils};

use super::server_utils::{BoxedFrameStream, ProxyHandlerBody};

pub async fn serve_file(
    location: &str,
    new_path: &str,
    spa_mode: bool,
    forbidden_dir: bool,
) -> Response<ProxyHandlerBody> {
    let path = format!("{}{}", utils::remove_last_slash(location), new_path);
    let mut file_path = sanitize_path(&path);

    // Serve Single Page Application
    if spa_mode {
        let spa_file = if file_path.is_file() {
            file_path
        } else {
            PathBuf::from(location).join("index.html")
        };

        tracing::info!("Serve SPA : {}", path);
        return match open_file(&spa_file).await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!("Serving file Error: {}", err);
                http_response::not_found()
            }
        };
    }

    tracing::info!("Serve static file : {}", path);

    if file_path.is_dir() {
        // Try to open index.html.
        file_path.push("index.html");
        return match open_file(&file_path).await {
            Ok(resp) => resp,
            // Default forbidden response if the path is a dir.
            Err(_) => {
                if !forbidden_dir {
                    return display_directory_content(&mut file_path, new_path).await;
                }
                http_response::forbidden()
            }
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

async fn display_directory_content(
    file_path: &mut PathBuf,
    current_path: &str,
) -> Response<ProxyHandlerBody> {
    file_path.pop(); // Remove index.html
    let mut dir = tokio::fs::read_dir(file_path).await.unwrap();
    let mut html = vec![format!(
        "<html>\
        <head><title>Index of {current_path}</title></head>\
        <body style='margin-top: 25px;\
        font-family: sans-serif;'>
        <h1>Index of {current_path}</h1>\
        <hr/>
        <table style='width:100%; text-align: left; table-layout: fixed;'>\
        <tr><th>Name</th><th>Last modified</th><th>Size</th></tr>",
    )];

    while let Some(entry) = dir.next_entry().await.unwrap() {
        let path = entry.path();
        let metadata = tokio::fs::metadata(&path).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        // get and format last modified.
        let modified = metadata.modified().unwrap();
        let datetime = OffsetDateTime::from(modified);
        let format =
            format_description::parse("[day]-[month repr:short]-[year] [hour]:[minute]:[second]")
                .unwrap();
        let last_modif = datetime.format(&format).unwrap();
        // get and format file size.
        let size = utils::format_size(metadata.len());

        html.push(format!(
            "<tr>\
            <td><a href='{file_name}'>{file_name}</a></td>\
            <td>{last_modif}</td>\
            <td>{size}</td>\
            </tr>",
        ));
    }

    html.push(String::from("</table><hr/></body></html>"));
    let html = html.join("\n");
    Response::builder()
        .status(StatusCode::OK)
        .body(ProxyHandlerBody::Full(Full::from(html)))
        .unwrap()
}

async fn open_file(file_path: &PathBuf) -> Result<Response<ProxyHandlerBody>, std::io::Error> {
    match tokio::fs::File::open(file_path).await {
        Ok(file) => {
            let mime_type = mime_guess::from_path(file_path)
                .first_or_octet_stream()
                .to_string();

            let reader_stream = ReaderStream::new(file)
                .map_ok(Frame::data)
                .map_err(std::io::Error::other);
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
