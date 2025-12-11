mod handler;
mod serve_file;
pub mod server_utils;

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr};
use std::pin::Pin;
use std::time::Duration;
use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use hyper::service::service_fn;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioTimer;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use server_utils::welcome_server;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

use tokio_rustls::TlsAcceptor;
use tracing::info;

use crate::config::tls::{reload_certificates, IpcCerts, SniCertResolver, TlsConfig};
use crate::config::{self, InternalConfig, Locations, Options, TargetType};
use crate::ipc::{self, IpcMessage};
use crate::server::handler::ServerHandler;
use crate::utils::{drop_privileges, format_ip, QUARK_USER_AND_GROUP};
use crate::{load_balancing, logs};

pub async fn server_process() -> Result<(), Box<dyn std::error::Error>> {
    // Wait for parent init.
    let socket_path = ipc::get_socket_path();
    let mut stream = match ipc::connect_to_socket(&socket_path).await {
        Ok(stream) => stream,
        Err(e) => {
            println!("Failed to connect to parent process: {e}");
            std::process::exit(1);
        }
    };

    // Get the InternalConfig from the parent process.
    let message_sc = ipc::receive_ipc_message::<InternalConfig>(&mut stream).await?;
    let internal_config = message_sc.payload;

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

    init_servers(internal_config, tls_certs, tx).await?;

    Ok(())
}

async fn init_servers(
    service_config: InternalConfig,
    tls_certs: Arc<HashMap<u16, Vec<IpcCerts>>>,
    tx: tokio::sync::broadcast::Sender<Arc<IpcMessage<Vec<IpcCerts>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting server");

    // List of servers to start.
    let mut servers: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = Vec::new();

    let http_builder = build_http(&service_config.global);
    let http = Arc::new(http_builder);
    let client = Arc::new(Client::builder(TokioExecutor::new()).build_http());
    let max_conns = Arc::new(tokio::sync::Semaphore::new(service_config.global.max_conn));
    let max_req = Arc::new(tokio::sync::Semaphore::new(service_config.global.max_req));
    let default_backlog = service_config.global.backlog;

    #[cfg(debug_assertions)]
    println!("Config: {:#?}", service_config.servers);

    // If no servers are defined, start a welcome server.
    // This usually happens when the config file is empty, especially right
    // after the server is installed for the first time.
    if service_config.empty {
        tracing::warn!("No services defined in the config file. Starting a welcome server.");
        tracing::warn!("Don't keep this server running in production without configuration!");
        welcome_server(http.clone()).await;
        return Ok(());
    }

    let lb_config = generate_loadbalancing_config(&service_config.servers);

    // Build a server for each port defined in the config file.
    for (_, server) in service_config.servers {
        let http = Arc::clone(&http);
        let client = Arc::clone(&client);
        let max_conns = Arc::clone(&max_conns);
        let max_req = Arc::clone(&max_req);
        let lb_config = Arc::clone(&lb_config);
        let tx = tx.clone();

        let server_params = Arc::new(server.params);
        let server_handler =
            handler::ServerHandler::builder(server_params, lb_config, max_req, client);

        // Declare https server if tls is enabled in the server config.
        if let Some(_tls) = &server.tls {
            // Clone arcs for the next asynvc task.
            let http = Arc::clone(&http);
            let max_conns = Arc::clone(&max_conns);
            let server_handler = Arc::clone(&server_handler);
            let tls_certs = Arc::clone(&tls_certs).clone();

            let https_server_config = HttpsServerConfig {
                port: server.https_port,
                default_backlog,
                handshake_timeout: service_config.global.tls_handshake_timeout,
            };

            let https_server = https_server(
                https_server_config,
                tx,
                tls_certs,
                max_conns,
                http,
                server_handler,
            );

            servers.push(Box::pin(https_server));
        }

        // Default http server. (Always enabled)
        let http_server = http_server(
            server.port,
            default_backlog,
            max_conns,
            http,
            server_handler,
        );

        servers.push(Box::pin(http_server));
    }

    // Drop privileges from root to "quark" user.
    // If we are not root, it wont do anything.
    match drop_privileges(QUARK_USER_AND_GROUP) {
        Ok(msg) => tracing::warn!("{}", msg),
        Err(err) => return Err(err),
    }

    // Start all the servers.
    join_all(servers).await;

    Ok(())
}

fn build_http(global_config: &config::Global) -> Builder<TokioExecutor> {
    let mut http_builder = Builder::new(TokioExecutor::new());

    http_builder
        .http1()
        .keep_alive(global_config.keepalive)
        .header_read_timeout(Duration::from_secs(global_config.http_header_timeout))
        .timer(TokioTimer::new());

    http_builder
        .http2()
        .keep_alive_interval(if global_config.keepalive {
            Some(Duration::from_secs(global_config.keepalive_interval))
        } else {
            None
        })
        .keep_alive_timeout(Duration::from_secs(global_config.keepalive_timeout))
        .timer(TokioTimer::new());

    http_builder
}

fn generate_loadbalancing_config(
    servers: &HashMap<String, config::Server>,
) -> Arc<load_balancing::LoadBalancerConfig> {
    let mut targets: Vec<&Locations> = Vec::new();
    for (_, server) in servers.iter() {
        for (_, target) in server.params.targets.iter() {
            match target {
                TargetType::Location(location) if location.algo.is_some() => {
                    targets.push(location);
                }
                _ => (),
            }
        }
    }

    load_balancing::LoadBalancerConfig::new(targets)
}

struct PlainAcceptor;
struct TlsAcceptorWrapper {
    acceptor: TlsAcceptor,
    handshake_timeout: u64,
}

trait StreamAcceptor: Send + Sync + 'static {
    type Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static;
    fn accept(
        &self,
        stream: tokio::net::TcpStream,
    ) -> impl Future<Output = Result<Self::Stream, std::io::Error>> + Send;
    fn protocol(&self) -> &'static str;
}

impl StreamAcceptor for PlainAcceptor {
    type Stream = tokio::net::TcpStream;
    async fn accept(&self, stream: tokio::net::TcpStream) -> Result<Self::Stream, std::io::Error> {
        Ok(stream)
    }
    fn protocol(&self) -> &'static str {
        "http"
    }
}

impl StreamAcceptor for TlsAcceptorWrapper {
    type Stream = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    async fn accept(&self, stream: tokio::net::TcpStream) -> Result<Self::Stream, std::io::Error> {
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(self.handshake_timeout),
            self.acceptor.accept(stream),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => Err(std::io::ErrorKind::TimedOut.into()),
        }
    }
    fn protocol(&self) -> &'static str {
        "https"
    }
}

fn run_server<A: StreamAcceptor>(
    port: u16,
    default_backlog: i32,
    max_conns: Arc<tokio::sync::Semaphore>,
    http: Arc<Builder<TokioExecutor>>,
    server_handler: Arc<ServerHandler>,
    acceptor: Arc<A>,
) -> impl Future<Output = ()> {
    let listener = build_tcp_listener(port, default_backlog);
    async move {
        loop {
            let res = listener.accept().await;
            let (stream, address) = match res {
                Ok(res) => res,
                Err(err) => {
                    tracing::error!("failed to accept connection: {err:#}");
                    continue;
                }
            };

            let client_ip = format_ip(address.ip());
            let acceptor = acceptor.clone();
            let max_conns = Arc::clone(&max_conns);
            let server_handler = Arc::clone(&server_handler);
            let http = http.clone();

            tokio::task::spawn(async move {
                let protocol = acceptor.protocol();
                let service = service_fn(move |req| {
                    let server_handler = Arc::clone(&server_handler);
                    let client_ip = client_ip.clone();
                    async move { server_handler.handle(req, client_ip, protocol).await }
                });

                let _permit = match max_conns.try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::error!("Too many connection. Connection closed.");
                        return;
                    }
                };

                let stream = match acceptor.accept(stream).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::error!("failed to perform TLS handshake: {err:#}");
                        return;
                    }
                };

                if let Err(err) = http.serve_connection(TokioIo::new(stream), service).await {
                    tracing::error!("failed to serve connection: {err:#}");
                }
            });
        }
    }
}

struct HttpsServerConfig {
    port: u16,
    default_backlog: i32,
    handshake_timeout: u64,
}

async fn https_server(
    config: HttpsServerConfig,
    tx: tokio::sync::broadcast::Sender<Arc<IpcMessage<Vec<IpcCerts>>>>,
    tls_certs: Arc<HashMap<u16, Vec<IpcCerts>>>,
    max_conns: Arc<tokio::sync::Semaphore>,
    http: Arc<Builder<TokioExecutor>>,
    server_handler: Arc<ServerHandler>,
) {
    let tls_acceptor = build_tls_acceptor_with_reload(config.port, tx, tls_certs).await;
    let acceptor = Arc::new(TlsAcceptorWrapper {
        acceptor: tls_acceptor,
        handshake_timeout: config.handshake_timeout,
    });

    run_server(
        config.port,
        config.default_backlog,
        max_conns,
        http,
        server_handler,
        acceptor,
    )
    .await;
}

async fn http_server(
    port: u16,
    default_backlog: i32,
    max_conns: Arc<tokio::sync::Semaphore>,
    http: Arc<Builder<TokioExecutor>>,
    server_handler: Arc<ServerHandler>,
) {
    let acceptor = Arc::new(PlainAcceptor);
    run_server(
        port,
        default_backlog,
        max_conns,
        http,
        server_handler,
        acceptor,
    )
    .await;
}

async fn build_tls_acceptor_with_reload(
    port: u16,
    tx: tokio::sync::broadcast::Sender<Arc<IpcMessage<Vec<IpcCerts>>>>,
    tls_certs: Arc<HashMap<u16, Vec<IpcCerts>>>,
) -> TlsAcceptor {
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
                info!("New certificates for port {}", port);
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
    TlsAcceptor::from(Arc::new(server_config))
}

fn build_tcp_listener(port: u16, backlog: i32) -> TcpListener {
    // Build TCP Socket and Socket Address.
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)).unwrap();
    let socket_addr: SocketAddr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
    // Allow IPv4 connections.
    socket.set_only_v6(false).unwrap();
    // Allow reuse of the address.
    socket.set_reuse_address(true).unwrap();
    // Define that the socket is non-blocking. Otherwise tokio can't accept it.
    socket.set_nonblocking(true).unwrap();
    // Bind the socket to the address.
    socket.bind(&socket_addr.into()).unwrap();
    // Define the backlog.
    socket.listen(backlog).unwrap();
    // Create and return the listener.
    info!("Server listening on port {}", port);
    TcpListener::from_std(socket.into()).unwrap()
}
