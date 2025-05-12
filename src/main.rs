mod config;
mod proxy_handler;

use std::{
    fs::{self},
    io::{self},
    net::SocketAddr,
    sync::Arc,
};

use config::ServiceConfig;
use hyper::{server, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    ServerConfig,
};
use tokio::net::TcpListener;

use argh::FromArgs;
use tokio_rustls::TlsAcceptor;

#[derive(FromArgs)]
#[argh(description = "certificates")]
struct Options {
    /// cert file
    #[argh(option, short = 'c')]
    config: String,
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options: Options = argh::from_env();

    // Temporary.
    let options_config = options.config.clone();

    println!("Starting server");

    // Load TOML server config.
    let server_config = config::get_toml_config(options.config);

    let service_config = ServiceConfig::build_from(options_config);

    println!("\n\n{:?}", service_config);

    let server_addr: SocketAddr = ([127, 0, 0, 1], 8080).into();

    // Test server
    let target_addr: SocketAddr = ([127, 0, 0, 1], 1234).into();

    let target_addr_clone = target_addr;

    let listener = TcpListener::bind(server_addr).await?;

    println!("Listening on http://{}", server_addr);
    println!("Proxying on http://{}", target_addr);

    // Load public certificate from config file.
    let certs = load_certs(
        &server_config.services["server1"]
            .tls
            .as_ref()
            .unwrap()
            .certificate,
    )?;
    // Load private key from config file.
    let key = load_private_key(&server_config.services["server1"].tls.as_ref().unwrap().key)?;

    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("Bad certificate/key");
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));

    loop {
        let (stream, _) = listener.accept().await?;

        let acceptor = tls_acceptor.clone();

        // This is the `Service` that will handle the connection.
        // `service_fn` is a helper to convert a function that
        // returns a Response into a `Service`.
        let service = service_fn(move |req| proxy_handler::proxy_handler(req, target_addr_clone));

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
