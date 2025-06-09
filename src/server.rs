use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use hyper::service::service_fn;
use hyper_util::client::legacy::Client;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

use tokio_rustls::TlsAcceptor;
use tracing::info;

use crate::config::tls::{reload_certificates, IpcCerts, SniCertResolver, TlsConfig};
use crate::config::{Options, ServiceConfig};
use crate::ipc::{self, IpcMessage, QUARK_SOCKET_PATH};
use crate::utils::{self, format_ip};
use crate::{logs, proxy_handler};

pub async fn server_process() -> Result<(), Box<dyn std::error::Error>> {
    // Wait for parent init.
    let mut stream = match ipc::connect_to_socket(QUARK_SOCKET_PATH).await {
        Ok(stream) => stream,
        Err(e) => {
            println!("Failed to connect to parent process: {}", e);
            std::process::exit(1);
        }
    };
    // Get the size of the config from the parent process.

    let message_sc = ipc::receive_ipc_message::<ServiceConfig>(&mut stream).await?;

    let service_config = message_sc.payload;

    // Get the certs from the parent process.
    let message_certs =
        ipc::receive_ipc_message::<HashMap<u16, Vec<IpcCerts>>>(&mut stream).await?;
    let tls_certs = message_certs.payload;
    let tls_certs = Arc::new(tls_certs);

    // Watch for certificates changes.
    let (tx, _) = tokio::sync::broadcast::channel::<Arc<IpcMessage<Vec<IpcCerts>>>>(16);
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(msg) = ipc::receive_ipc_message::<Vec<IpcCerts>>(&mut stream).await {
                let msg = Arc::new(msg);
                tx_clone.send(msg).unwrap();
            }
        }
    });

    // Get options from command line.
    let options: Options = argh::from_env();

    // Init logs. Declare a var to keep the guard alive in this scope.
    let _guard = logs::start_logs(options.logs);

    info!("Starting server");

    // List of servers to start.
    let mut servers = Vec::new();

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
        let tx = tx.clone();

        let tls_certs = Arc::clone(&tls_certs).clone();

        let listener = TcpListener::from_std(socket.into()).unwrap();
        info!("Server listening on port {}", port);

        let service = async move {
            match server.tls {
                // If server has TLS configuration, create a server for https.
                Some(_tls) => {
                    // Start the tls config.

                    let mut rx = tx.subscribe();

                    let tls_certs = tls_certs.get(&port).unwrap();

                    let tls_config = Arc::new(tokio::sync::Mutex::new(TlsConfig::new(tls_certs)));
                    let ck_list = {
                        let mut guard = tls_config.lock().await;
                        Arc::new(guard.get_certified_key_list())
                    };

                    // Spawn a task to watch for certificates changes.
                    let port_string = port.to_string();
                    let ck_list_clone = ck_list.clone();
                    tokio::spawn(async move {
                        while let Ok(msg) = rx.recv().await {
                            if msg.key.as_ref().unwrap() == &port_string {
                                println!("[TLS] New certificate for port {}", port);
                                msg.payload.iter().for_each(|cert| {
                                    reload_certificates(cert, ck_list_clone.clone());
                                })
                            }
                        }
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
