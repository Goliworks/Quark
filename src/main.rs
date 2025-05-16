mod config;
mod proxy_handler;

use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use config::tls::TlsConfig;
use config::ServiceConfig;
use hyper::service::service_fn;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
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

        let targets = Arc::new(server.targets);

        let service = async move {
            let listener = TcpListener::bind(server_addr).await.unwrap();

            match server.tls {
                // If server has TLS configuration, create a server for https.
                Some(tls) => {
                    // custom tls config.
                    let tls_config = TlsConfig::new(&tls).get_tls_config();

                    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

                    loop {
                        let (stream, _) = listener.accept().await.unwrap();

                        let acceptor = tls_acceptor.clone();

                        let targets = Arc::clone(&targets);

                        // This is the `Service` that will handle the connection.
                        // returns a Response into a `Service`.
                        let service = service_fn(move |req| {
                            proxy_handler::proxy_handler(req, targets.clone())
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
                // Otherwise, create a default server for http.
                None => loop {
                    let targets = Arc::clone(&targets);
                    let (stream, _) = listener.accept().await.unwrap();
                    let service =
                        service_fn(move |req| proxy_handler::proxy_handler(req, targets.clone()));
                    tokio::task::spawn(async move {
                        if let Err(err) = Builder::new(TokioExecutor::new())
                            .serve_connection(TokioIo::new(stream), service)
                            .await
                        {
                            eprintln!("failed to serve connection: {err:#}");
                        }
                    });
                },
            }
        };

        // Add the server to the list.
        servers.push(service);
    }

    // Start all the servers.
    join_all(servers).await;

    Ok(())
}
