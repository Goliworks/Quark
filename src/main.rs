mod config;
mod http_response;
mod proxy_handler;
mod serve_file;
mod utils;

use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use config::tls::{SniCertResolver, TlsConfig};
use config::ServiceConfig;
use hyper::service::service_fn;
use hyper_util::client::legacy::Client;
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

    let http = Arc::new(Builder::new(TokioExecutor::new()));
    let client = Arc::new(Client::builder(TokioExecutor::new()).build_http());
    let max_conns = Arc::new(tokio::sync::Semaphore::new(service_config.global.max_conn));
    let max_req = Arc::new(tokio::sync::Semaphore::new(service_config.global.max_req));

    // Build a server for each port defined in the config file.
    for (port, server) in service_config.servers {
        println!("Server listening on port {}", port);

        let server_addr: SocketAddr = ([0, 0, 0, 0], port).into();

        let server_params = Arc::new(server.params);

        let http = Arc::clone(&http);
        let client = Arc::clone(&client);
        let max_conns = Arc::clone(&max_conns);
        let max_req = Arc::clone(&max_req);

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
                        let client = Arc::clone(&client);
                        let permit = match max_conns.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                eprintln!("Too many TLS connection. Connection closed.");
                                continue;
                            }
                        };

                        let res = listener.accept().await;
                        let (stream, _) = match res {
                            Ok(res) => res,
                            Err(err) => {
                                eprintln!("failed to accept connection: {err:#}");
                                continue;
                            }
                        };

                        let acceptor = tls_acceptor.clone();

                        let server_params = Arc::clone(&server_params);

                        let max_req = max_req.clone();
                        // This is the `Service` that will handle the connection.
                        // returns a Response into a `Service`.
                        let service = service_fn(move |req| {
                            proxy_handler::proxy_handler(
                                req,
                                server_params.clone(),
                                max_req.clone(),
                                client.clone(),
                            )
                        });

                        let http = http.clone();
                        tokio::task::spawn(async move {
                            let stream = match acceptor.accept(stream).await {
                                Ok(stream) => stream,
                                Err(err) => {
                                    eprintln!("failed to perform tls handshake: {err:#}");
                                    return;
                                }
                            };
                            if let Err(err) =
                                http.serve_connection(TokioIo::new(stream), service).await
                            {
                                eprintln!("failed to serve connection: {err:#}");
                            }
                            drop(permit);
                        });
                    }
                }
                // Otherwise, create a default server for http.
                None => loop {
                    let client = Arc::clone(&client);
                    let permit = match max_conns.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            eprintln!("Too many TLS connection. Connection closed.");
                            continue;
                        }
                    };

                    let server_params = Arc::clone(&server_params);

                    let res = listener.accept().await;
                    let (stream, _) = match res {
                        Ok(res) => res,
                        Err(err) => {
                            eprintln!("failed to accept connection: {err:#}");
                            continue;
                        }
                    };

                    let max_req = max_req.clone();
                    let service = service_fn(move |req| {
                        proxy_handler::proxy_handler(
                            req,
                            server_params.clone(),
                            max_req.clone(),
                            client.clone(),
                        )
                    });
                    let http = http.clone();
                    tokio::task::spawn(async move {
                        let permit = permit;
                        if let Err(err) = http.serve_connection(TokioIo::new(stream), service).await
                        {
                            eprintln!("failed to serve connection: {err:#}");
                        }
                        drop(permit);
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
