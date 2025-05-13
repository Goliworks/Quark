mod config;
mod proxy_handler;

use std::{
    fs::{self},
    io::{self},
    net::SocketAddr,
    sync::Arc,
};

use ::futures::future::join_all;
use config::ServiceConfig;
use hyper::service::service_fn;
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
    /// config file path.
    #[argh(option, short = 'c')]
    config: String,
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options: Options = argh::from_env();
    let options_config = options.config.clone();

    println!("Starting server");

    // Liste of servers to start.
    let mut servers = Vec::new();

    // Read config file and build de server configuration via the path defined in options on startup.
    let service_config = ServiceConfig::build_from(options_config);

    println!("\n\n{:?}\n\n", service_config);

    // Build a server for each port defined in the config file.
    for (port, server) in service_config.servers {
        println!("Server listening on port {}", port);

        let server_addr: SocketAddr = ([127, 0, 0, 1], port).into();

        let service = async move {
            let listener = TcpListener::bind(server_addr).await.unwrap();

            match server.tls {
                // If server has TLS configuration, create a server for https.
                Some(tls) => {
                    // Temporary use the first certificate found. Need to implement SNI later.
                    let certs = load_certs(&tls[0].cert).unwrap();
                    let key = load_private_key(&tls[0].key).unwrap();

                    let mut server_config = ServerConfig::builder()
                        .with_no_client_auth()
                        .with_single_cert(certs, key)
                        .expect("Bad certificate/key");
                    server_config.alpn_protocols =
                        vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

                    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));

                    // TEST ! TO remove !
                    let target_addr: SocketAddr = ([127, 0, 0, 1], 8091).into();

                    loop {
                        let (stream, _) = listener.accept().await.unwrap();

                        let acceptor = tls_acceptor.clone();

                        // This is the `Service` that will handle the connection.
                        // returns a Response into a `Service`.
                        let service =
                            service_fn(move |req| proxy_handler::proxy_handler(req, target_addr));

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
                // Otherwise, create a default server for http.
                None => {
                    // TEST ! TO remove !
                    let target_addr: SocketAddr = ([127, 0, 0, 1], 8091).into();

                    loop {
                        let (stream, _) = listener.accept().await.unwrap();
                        let service =
                            service_fn(move |req| proxy_handler::proxy_handler(req, target_addr));
                        if let Err(err) = Builder::new(TokioExecutor::new())
                            .serve_connection(TokioIo::new(stream), service)
                            .await
                        {
                            eprintln!("failed to serve connection: {err:#}");
                        }
                    }
                }
            }
        };

        // Add the server to the list.
        servers.push(service);
    }

    // Start all the servers.
    join_all(servers).await;

    Ok(())
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
