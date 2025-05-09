use std::{
    fs::{self},
    io::{self},
    net::SocketAddr,
    sync::Arc,
};

use hyper::service::service_fn;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    ServerConfig,
};
use tokio::net::{TcpListener, TcpStream};

use argh::FromArgs;
use tokio_rustls::TlsAcceptor;

#[derive(FromArgs)]
#[argh(description = "certificates")]
struct Options {
    /// cert file
    #[argh(option, short = 'c')]
    cert: String,

    /// key file
    #[argh(option, short = 'k')]
    key: String,
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options: Options = argh::from_env();

    println!("Starting server");

    let server_addr: SocketAddr = ([127, 0, 0, 1], 8080).into();

    // Test server
    let target_addr: SocketAddr = ([127, 0, 0, 1], 1234).into();

    let target_addr_clone = target_addr;

    let listener = TcpListener::bind(server_addr).await?;

    println!("Listening on http://{}", server_addr);
    println!("Proxying on http://{}", target_addr);

    // Load public certificate.
    let certs = load_certs(&options.cert)?;
    // Load private key.
    let key = load_private_key(&options.key)?;

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("Bad certificate/key");

    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));

    loop {
        let (stream, _) = listener.accept().await?;

        let acceptor = tls_acceptor.clone();

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
            let stream = match acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(err) => {
                    eprintln!("failed to perform tls handshake: {err:#}");
                    return;
                }
            };
            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                eprintln!("failed to serve connection: {err:#}");
            }
        });
    }
}

// Load public certificate from file.
fn load_certs(filename: &str) -> io::Result<Vec<CertificateDer<'static>>> {
    // Open certificate file.
    let certfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    rustls_pemfile::certs(&mut reader).collect()
}

// Load private key from file.
fn load_private_key(filename: &str) -> io::Result<PrivateKeyDer<'static>> {
    // Open keyfile.
    let keyfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(keyfile);

    // Load and return a single private key.
    rustls_pemfile::private_key(&mut reader).map(|key| key.unwrap())
}
