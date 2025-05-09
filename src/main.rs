use std::net::SocketAddr;

use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting server");

    let server_addr: SocketAddr = ([127, 0, 0, 1], 8080).into();

    // Test server
    let target_addr: SocketAddr = ([127, 0, 0, 1], 1234).into();

    let target_addr_clone = target_addr;

    let listener = TcpListener::bind(server_addr).await?;

    println!("Listening on http://{}", server_addr);
    println!("Proxying on http://{}", target_addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);

        // This is the `Service` that will handle the connection.
        // `service_fn` is a helper to convert a function that
        // returns a Response into a `Service`.
        let service = service_fn(move |mut req| {
            let uri_string = format!(
                "http://{}{}",
                target_addr_clone,
                req.uri()
                    .path_and_query()
                    .map(|x| x.as_str())
                    .unwrap_or("/")
            );
            let uri = uri_string.parse().unwrap();
            *req.uri_mut() = uri;

            let host = req.uri().host().expect("uri has no host");
            let port = req.uri().port_u16().unwrap_or(80);
            let addr = format!("{}:{}", host, port);

            async move {
                let client_stream = TcpStream::connect(addr).await.unwrap();
                let io = TokioIo::new(client_stream);

                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::task::spawn(async move {
                    if let Err(err) = conn.await {
                        println!("Connection failed: {:?}", err);
                    }
                });

                sender.send_request(req).await
            }
        });

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                println!("Failed to serve the connection: {:?}", err);
            }
        });
    }
}
