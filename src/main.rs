mod config;
mod http_response;
mod ipc;
mod logs;
mod proxy_handler;
mod serve_file;
mod utils;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use config::tls::{self, reload_certificates, IpcCerts, SniCertResolver, TlsConfig};
use config::ServiceConfig;
use hyper::service::service_fn;
use hyper_util::client::legacy::Client;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use ipc::IpcMessage;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

use argh::FromArgs;
use tokio::sync::Mutex;
use tokio::time::sleep;
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

    /// run as child process
    #[argh(switch)]
    _child_process: bool,
}

const QUARK_SOCKET_PATH: &str = "/tmp/quark.sock";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // If the child process flag is set, run the server as a child process.
    if std::env::args().any(|arg| arg == "--child-process") {
        return server_process().await;
    }

    // If not, run a new process flagged as a child process.

    // Clean the socket file if it exists.
    if std::path::Path::new(QUARK_SOCKET_PATH).exists() {
        std::fs::remove_file(QUARK_SOCKET_PATH)?;
    }

    // Take the rest of the arguments and pass them to the child process.
    let mut child_args: Vec<String> = std::env::args().skip(1).collect();
    child_args.insert(0, "--child-process".to_string());

    // Run the child process.
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .args(child_args)
        .spawn()?;

    // Run the main process.
    main_process().await?;
    child.wait()?;
    Ok(())
}

async fn main_process() -> Result<(), Box<dyn std::error::Error>> {
    // Create a unix socket listener.
    let listener = tokio::net::UnixListener::bind(QUARK_SOCKET_PATH)?;
    println!("[Parent] Waiting for connection");
    let (stream, _) = listener.accept().await?;
    let stream = Arc::new(Mutex::new(stream));
    println!("[Parent] Connection accepted");

    // Get options from command line.
    let options: Options = argh::from_env();
    // Load the config file.
    let service_config = ServiceConfig::build_from(options.config);

    let mut paths_to_watch_list: HashMap<u16, Vec<PathBuf>> = HashMap::new();
    let mut cert_list: HashMap<u16, Vec<IpcCerts>> = HashMap::new();
    let mut tls_servers: HashMap<u16, Vec<config::TlsCertificate>> = HashMap::new();

    for (port, server) in &service_config.servers {
        if let Some(tls_certs) = &server.tls {
            tls_servers.insert(*port, tls_certs.clone());
            println!("[Parent] Server {} is configured with TLS", port);
            println!("[Parent] tls {:#?}", tls_certs);
            for cert in tls_certs {
                // Add the certificates path to the list of paths to watch.
                let path = Path::new(&cert.cert);
                let directory = path.parent().unwrap();
                let pathbuf = directory.to_path_buf();
                let paths_to_watch = paths_to_watch_list.entry(*port).or_default();
                if !paths_to_watch.contains(&pathbuf) {
                    paths_to_watch.push(pathbuf);
                }
                // Read the certificate and the key.
                let certfile = tokio::fs::read(cert.cert.as_str()).await?;
                let keyfile = tokio::fs::read(cert.key.as_str()).await?;
                let certs = IpcCerts {
                    cert: certfile,
                    key: keyfile,
                };
                cert_list.entry(*port).or_default().push(certs);
            }
        }
    }

    println!("[Parent] paths to watch {:#?}", paths_to_watch_list);

    // Send the config to the child process.
    let message = ipc::IpcMessage {
        kind: "config".to_string(),
        key: None,
        payload: service_config,
    };
    ipc::send_ipc_message(stream.clone(), message).await?;

    // Send the certs to the child process.
    let message = ipc::IpcMessage {
        kind: "certs".to_string(),
        key: None,
        payload: cert_list,
    };
    ipc::send_ipc_message(stream.clone(), message).await?;

    // Watch certificates
    for (port, paths_to_watch) in paths_to_watch_list {
        let stream = Arc::clone(&stream);
        let certs = tls_servers.get(&port).unwrap().clone();
        tokio::task::spawn(async move {
            tls::watch_certs(&paths_to_watch, port, stream, certs).await;
        });
    }
    Ok(())
}

async fn server_process() -> Result<(), Box<dyn std::error::Error>> {
    // Wait for parent init.
    sleep(Duration::from_millis(100)).await;
    // Connect to the parent process.
    let mut stream = tokio::net::UnixStream::connect(QUARK_SOCKET_PATH).await?;
    // Get the size of the config from the parent process.

    let message_sc = ipc::receive_ipc_message::<ServiceConfig>(&mut stream).await?;

    let service_config = message_sc.payload;

    // Get the certs from the parent process.
    let message_certs =
        ipc::receive_ipc_message::<HashMap<u16, Vec<IpcCerts>>>(&mut stream).await?;
    let tls_certs = message_certs.payload;
    let tls_certs = Arc::new(tls_certs);

    // Watch for certificates changes.
    let (tx, _) = tokio::sync::broadcast::channel::<Arc<IpcMessage<Vec<IpcCerts>>>>(10);
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

    // Init logs.
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
