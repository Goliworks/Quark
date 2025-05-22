mod config;
mod error;
mod proxy_handler;

use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use config::tls::{SniCertResolver, TlsConfig};
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

        let server_params = Arc::new(server.params);

        let service = async move {
            let listener = TcpListener::bind(server_addr).await.unwrap();

            match server.tls {
                // If server has TLS configuration, create a server for https.
                Some(tls) => {
                    // Start the tls config.
                    let tls_config = Arc::new(tokio::sync::Mutex::new(TlsConfig::new(tls)));
                    let ck_list = {
                        let mut guard = tls_config.lock().await;
                        Arc::new(guard.get_certified_key_list())
                    };

                    let tls_config_clone = Arc::clone(&tls_config);
                    let ck_list_clone = Arc::clone(&ck_list);

                    // Start to watch for certificates changes.
                    tokio::task::spawn(async move {
                        let guard = tls_config_clone.lock().await;
                        guard.watch_certs(ck_list_clone).await;
                    });

                    // Generate the sni resolver pass it to the tls_config
                    // to get the rustls server config.
                    let resolver = SniCertResolver::new(ck_list);
                    let server_config = {
                        let guard = tls_config.lock().await;
                        guard.get_tls_config(resolver)
                    };

                    // Create the tls acceptor with the rustls server config.
                    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));

                    loop {
                        let (stream, _) = listener.accept().await.unwrap();

                        let acceptor = tls_acceptor.clone();

                        let server_params = Arc::clone(&server_params);

                        // This is the `Service` that will handle the connection.
                        // returns a Response into a `Service`.
                        let service = service_fn(move |req| {
                            proxy_handler::proxy_handler(req, server_params.clone())
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
                    let server_params = Arc::clone(&server_params);
                    let (stream, _) = listener.accept().await.unwrap();
                    let service = service_fn(move |req| {
                        proxy_handler::proxy_handler(req, server_params.clone())
                    });
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
