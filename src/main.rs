mod config;
mod http_response;
mod logs;
mod proxy_handler;
mod serve_file;
mod utils;

use std::net::{IpAddr, Ipv6Addr};
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
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

use argh::FromArgs;
use tokio_rustls::TlsAcceptor;
use tracing::info;
use utils::{format_ip, DEFAULT_CONFIG_FILE_PATH, DEFAULT_LOG_PATH};

#[derive(FromArgs)]
#[argh(description = "certificates")]
struct Options {
    /// config file path.
    #[argh(option, short = 'c', default = "DEFAULT_CONFIG_FILE_PATH.to_string()")]
    config: String,
    /// logs directory path
    #[argh(option, short = 'l', default = "DEFAULT_LOG_PATH.to_string()")]
    logs: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get options from command line.
    let options: Options = argh::from_env();

    // Init logs.
    let _guard = logs::start_logs(options.logs);

    info!("Starting server");

    // List of servers to start.
    let mut servers = Vec::new();

    // Read config file and build de server configuration via the path defined in options on startup.
    let service_config = ServiceConfig::build_from(options.config);

    let http = Arc::new(Builder::new(TokioExecutor::new()));
    let client = Arc::new(Client::builder(TokioExecutor::new()).build_http());
    let max_conns = Arc::new(tokio::sync::Semaphore::new(service_config.global.max_conn));
    let max_req = Arc::new(tokio::sync::Semaphore::new(service_config.global.max_req));
    let default_backlog = service_config.global.backlog;

    // Build a server for each port defined in the config file.
    for (port, server) in service_config.servers {
        // Build TCP Socket and Socket Address.
        let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)).unwrap();
        let socket_addr: SocketAddr =
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port).into();
        // Allow IPv4 connections.
        socket.set_only_v6(false).unwrap();
        // Allow reuse of the address.
        socket.set_reuse_address(true).unwrap();
        // Define that the socket is non-blocking. Otherwise tokio can't accept it.
        socket.set_nonblocking(true).unwrap();
        // Bind the socket to the address.
        socket.bind(&socket_addr.into()).unwrap();
        // Define the backlog.
        socket.listen(default_backlog).unwrap();

        let server_params = Arc::new(server.params);

        let http = Arc::clone(&http);
        let client = Arc::clone(&client);
        let max_conns = Arc::clone(&max_conns);
        let max_req = Arc::clone(&max_req);

        let listener = TcpListener::from_std(socket.into()).unwrap();
        info!("Server listening on port {}", port);

        let service = async move {
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
                                tracing::error!("Too many TLS connection. Connection closed.");
                                continue;
                            }
                        };

                        let res = listener.accept().await;
                        let (stream, address) = match res {
                            Ok(res) => res,
                            Err(err) => {
                                tracing::error!("failed to accept connection: {err:#}");
                                continue;
                            }
                        };

                        let client_ip = format_ip(address.ip());

                        let acceptor = tls_acceptor.clone();

                        let server_params = Arc::clone(&server_params);

                        let max_req = max_req.clone();

                        // This service will handle the connection.
                        let service = service_fn(move |req| {
                            proxy_handler::proxy_handler(
                                req,
                                server_params.clone(),
                                max_req.clone(),
                                client.clone(),
                                client_ip.clone(),
                                "https",
                            )
                        });

                        let http = http.clone();
                        tokio::task::spawn(async move {
                            let stream = match acceptor.accept(stream).await {
                                Ok(stream) => stream,
                                Err(err) => {
                                    tracing::error!("failed to perform tls handshake: {err:#}");
                                    return;
                                }
                            };
                            if let Err(err) =
                                http.serve_connection(TokioIo::new(stream), service).await
                            {
                                tracing::error!("failed to serve connection: {err:#}");
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
                            tracing::error!("Too many TLS connection. Connection closed.");
                            continue;
                        }
                    };

                    let server_params = Arc::clone(&server_params);

                    let res = listener.accept().await;
                    let (stream, address) = match res {
                        Ok(res) => res,
                        Err(err) => {
                            tracing::error!("failed to accept connection: {err:#}");
                            continue;
                        }
                    };

                    let client_ip = format_ip(address.ip());

                    let max_req = max_req.clone();

                    // This service will handle the connection.
                    let service = service_fn(move |req| {
                        proxy_handler::proxy_handler(
                            req,
                            server_params.clone(),
                            max_req.clone(),
                            client.clone(),
                            client_ip.clone(),
                            "http",
                        )
                    });
                    let http = http.clone();
                    tokio::task::spawn(async move {
                        let permit = permit;
                        if let Err(err) = http.serve_connection(TokioIo::new(stream), service).await
                        {
                            tracing::error!("failed to serve connection: {err:#}");
                        }
                        drop(permit);
                    });
                },
            }
        };

        // Add the server to the list.
        servers.push(service);
    }

    // Drop privileges from root to www-data.
    // If we are not root, it wont do anything.
    match utils::drop_privileges("www-data") {
        Ok(msg) => tracing::warn!("{}", msg),
        Err(err) => return Err(err),
    }

    // Start all the servers.
    join_all(servers).await;

    Ok(())
}
