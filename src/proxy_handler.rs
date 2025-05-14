use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use http_body_util::Full;
use hyper::{
    body::{Bytes, Incoming},
    Request, Response,
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};

pub async fn proxy_handler(
    req: Request<Incoming>,
    targets: Arc<HashMap<String, SocketAddr>>,
) -> Result<Response<Incoming>, hyper_util::client::legacy::Error> {
    // Get the domain (and remove port) from host.
    let domain = req.headers()["host"]
        .to_str()
        .unwrap()
        .split(':')
        .next()
        .unwrap();

    println!("{}", domain);
    println!("{:?}", targets);
    println!("ARC {}", Arc::strong_count(&targets));

    let target = targets.get(domain).unwrap();

    println!("{}", target);

    let uri_string = format!(
        "http://{}{}",
        target,
        req.uri()
            .path_and_query()
            .map(|x| x.as_str())
            .unwrap_or("/")
    );

    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();

    let (parts, _body) = req.into_parts();

    let mut new_req: Request<Full<Bytes>> = Request::builder()
        .method(parts.method)
        .uri(uri_string)
        .body(Full::from(""))
        .expect("request builder");

    *new_req.headers_mut() = parts.headers;

    let future = client.request(new_req);
    future.await
}
