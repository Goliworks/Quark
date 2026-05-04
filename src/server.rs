mod handler;
mod serve_file;
pub mod server_utils;

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv6Addr};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use std::{net::SocketAddr, sync::Arc};

use ::futures::future::join_all;
use dashmap::DashMap;
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

use tokio::signal::unix::{signal, SignalKind};
use tokio_rustls::TlsAcceptor;
use tracing::info;

use crate::config::tls::{reload_certificates, IpcCerts, SniCertResolver, TlsConfig};
use crate::config::{self, InternalConfig, Locations, Options, TargetType};
use crate::ipc::{self, IpcMessage};
use crate::middleware::ServerService;
use crate::server::handler::ServerHandler;
use crate::utils::{drop_privileges, format_ip, CACHED_CURRENT_TIME, QUARK_USER_AND_GROUP};
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
            match ipc::receive_ipc_message::<Vec<IpcCerts>>(&mut stream).await {
                Ok(msg) => {
                    let msg = Arc::new(msg);
                    let _ = tx_clone.send(msg);
                }
                Err(err) => {
                    tracing::error!("IPC stream error: {err:#}");
                    break;
                }
            }
        }
    });

    // Get options from command line.
    let options: Options = argh::from_env();
    // Init logs. Declare a var to keep the guard alive in this scope.
    let _guard = logs::start_logs(options.logs);

    check_sigterm();

    update_cached_time_worker();

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

        let limiter = service_config
            .global
            .max_conn_per_ip
            .map(|max_conn| Arc::new(ConnectionLimiter::new(max_conn)));

        // Declare https server if tls is enabled in the server config.
        if let Some(_tls) = &server.tls {
            // Clone arcs for the next asynvc task.
            let http = Arc::clone(&http);
            let max_conns = Arc::clone(&max_conns);
            let server_handler = Arc::clone(&server_handler);
            let tls_certs = Arc::clone(&tls_certs).clone();
            let limiter = limiter.clone();

            let https_config = HttpServerConfig {
                max_conns,
                http,
                server_handler,
                idle_timeout: service_config.global.idle_timeout,
                idle_check_interval: service_config.global.idle_check_interval,
                limiter,
            };

            let listener =
                build_tcp_listener(server.https_port, default_backlog).map_err(|err| {
                    tracing::error!("failed to create https listener: {err:#}");
                    err
                })?;

            let https_server = https_server(
                https_config,
                tx,
                tls_certs,
                service_config.global.tls_handshake_timeout,
                server.https_port,
                listener,
            );

            servers.push(Box::pin(https_server));
        }

        let http_config = HttpServerConfig {
            max_conns,
            http,
            server_handler,
            idle_timeout: service_config.global.idle_timeout,
            idle_check_interval: service_config.global.idle_check_interval,
            limiter,
        };

        let listener = build_tcp_listener(server.port, default_backlog).map_err(|err| {
            tracing::error!("failed to create http listener: {err:#}");
            err
        })?;
        // Default http server. (Always enabled)
        let http_server = http_server(http_config, listener);

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
        for (_, routes) in server.params.routes.iter() {
            for route in routes {
                match &route.target {
                    TargetType::Location(location) if location.algo.is_some() => {
                        targets.push(location);
                    }
                    _ => (),
                }
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

async fn run_server<A: StreamAcceptor>(
    config: HttpServerConfig,
    listener: TcpListener,
    acceptor: Arc<A>,
) {
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
        let ip_addr = address.ip();
        let acceptor = acceptor.clone();
        let max_conns = Arc::clone(&config.max_conns);
        let server_handler = Arc::clone(&config.server_handler);
        let limiter = config.limiter.clone();
        let http = config.http.clone();

        tokio::task::spawn(async move {
            // Limit ip only if defined in the config file.
            let _conn_guard = if let Some(ref limiter) = limiter {
                match limiter.try_acquire(ip_addr) {
                    Some(guard) => Some(guard),
                    None => {
                        tracing::warn!(ip = %ip_addr,
                                "Connection limit exceeded");
                        return;
                    }
                }
            } else {
                None
            };

            let _permit = match max_conns.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tracing::error!("Too many connection. Connection closed.");
                    return;
                }
            };

            let protocol = acceptor.protocol().to_string();
            let service = service_fn(move |req| {
                let server_handler = Arc::clone(&server_handler);
                let client_ip = client_ip.clone();
                let protocol = protocol.clone();
                let handler_params = handler::HandlerParams {
                    req,
                    client_ip,
                    scheme: protocol,
                };
                async move { server_handler.handle(handler_params).await }
            });
            let service = ServerService::new(service);

            let stream = match acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::error!("failed to perform TLS handshake: {err:#}");
                    return;
                }
            };

            let conn = http.serve_connection(TokioIo::new(stream), service.clone());
            tokio::pin!(conn);

            let mut check_interval =
                tokio::time::interval(Duration::from_secs(config.idle_check_interval));
            check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    res = conn.as_mut() => {
                        match res {
                            Ok(_) => {
                                tracing::info!("Connection closed");
                            },
                            Err(err) => {
                                tracing::error!("failed to serve connection: {err:#}");
                            }
                        }
                        break;
                    }
                    _ = check_interval.tick() => {
                        let idle_secs = service.seconds_since_last_activity();

                        if idle_secs >= config.idle_timeout {
                           tracing::warn!(
                                idle_seconds = idle_secs,
                                "Connection idle timeout, closing connection"
                           );

                            conn.as_mut().graceful_shutdown();
                            if tokio::time::timeout(
                                Duration::from_secs(5),
                                conn.as_mut()
                            ).await.is_err() {
                                tracing::warn!("Connection shutdown timeout");
                            }
                            break;
                        } else {
                            tracing::debug!(
                                idle_seconds = idle_secs,
                                "Connection idle"
                            );
                        }
                    }
                }
            }
        });
    }
}

fn check_sigterm() {
    tokio::spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        sigterm.recv().await;
        tracing::info!("[Child Process] Received SIGTERM, exiting");
        std::process::exit(0);
    });
}

struct HttpServerConfig {
    max_conns: Arc<tokio::sync::Semaphore>,
    http: Arc<Builder<TokioExecutor>>,
    server_handler: Arc<ServerHandler>,
    idle_timeout: u64,
    idle_check_interval: u64,
    limiter: Option<Arc<ConnectionLimiter>>,
}

async fn https_server(
    config: HttpServerConfig,
    tx: tokio::sync::broadcast::Sender<Arc<IpcMessage<Vec<IpcCerts>>>>,
    tls_certs: Arc<HashMap<u16, Vec<IpcCerts>>>,
    handshake_timeout: u64,
    port: u16,
    listener: TcpListener,
) {
    let tls_acceptor = build_tls_acceptor_with_reload(port, tx, tls_certs).await;
    let acceptor = Arc::new(TlsAcceptorWrapper {
        acceptor: tls_acceptor,
        handshake_timeout,
    });

    run_server(config, listener, acceptor).await;
}

async fn http_server(config: HttpServerConfig, listener: TcpListener) {
    let acceptor = Arc::new(PlainAcceptor);
    run_server(config, listener, acceptor).await;
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

fn build_tcp_listener(port: u16, backlog: i32) -> io::Result<TcpListener> {
    // Build TCP Socket and Socket Address.
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    let socket_addr: SocketAddr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
    // Allow IPv4 connections.
    socket.set_only_v6(false)?;
    // Allow reuse of the address.
    socket.set_reuse_address(true)?;
    // Define that the socket is non-blocking. Otherwise tokio can't accept it.
    socket.set_nonblocking(true)?;
    // Bind the socket to the address.
    socket.bind(&socket_addr.into())?;
    // Define the backlog.
    socket.listen(backlog)?;
    // Create and return the listener.
    info!("Server listening on port {}", port);
    TcpListener::from_std(socket.into())
}

#[derive(Clone)]
struct ConnectionLimiter {
    connections: Arc<DashMap<IpAddr, usize>>,
    max_conns: usize,
}

impl ConnectionLimiter {
    pub fn new(max_conns: usize) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            max_conns,
        }
    }

    pub fn try_acquire(&self, ip: IpAddr) -> Option<ConnectionGuard> {
        let mut entry = self.connections.entry(ip).or_insert(0);
        if *entry >= self.max_conns {
            tracing::warn!(ip = %ip, current = *entry, "IP connection limit reached");
            return None;
        }
        *entry += 1;
        tracing::debug!(ip = %ip, entry = *entry, "Connection acquired");
        Some(ConnectionGuard {
            ip,
            limiter: self.clone(),
        })
    }

    pub fn release(&self, ip: IpAddr) {
        self.connections.remove_if_mut(&ip, |_, count| {
            if *count <= 1 {
                tracing::debug!(ip = %ip, "Connection removed");
                true
            } else {
                *count -= 1;
                tracing::debug!(ip = %ip, remaining = *count, "Connection released");
                false
            }
        });
    }
}

struct ConnectionGuard {
    ip: IpAddr,
    limiter: ConnectionLimiter,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.limiter.release(self.ip);
    }
}

static TIME_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn update_cached_time_worker() {
    TIME_START.get_or_init(Instant::now);
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let start = TIME_START.get().unwrap();
        loop {
            interval.tick().await;
            CACHED_CURRENT_TIME.store(start.elapsed().as_secs(), Ordering::Relaxed);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr},
        sync::Arc,
        time::Duration,
    };

    use crate::server::ConnectionLimiter;

    #[test]
    fn connection_limiter_explicit_release() {
        let limiter = ConnectionLimiter::new(1);
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let _g = limiter.try_acquire(ip).unwrap();
        limiter.release(ip);
        assert!(limiter.try_acquire(ip).is_some());
    }

    #[test]
    fn connections_limiter_drop_on_panic() {
        let limiter = ConnectionLimiter::new(1);
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = limiter.try_acquire(ip).unwrap();
            panic!();
        }));

        let second_attempt = limiter.try_acquire(ip);
        assert!(
            second_attempt.is_some(),
            "The connection should be available"
        );

        drop(second_attempt);
        assert!(
            !limiter.connections.contains_key(&ip),
            "The IP address should have been removed"
        );
    }

    #[test]
    fn connection_limiter_ip_isolation() {
        let limiter = ConnectionLimiter::new(1);
        let ip1 = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let ip2 = IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8));
        let _g1 = limiter.try_acquire(ip1).unwrap();
        assert!(limiter.try_acquire(ip2).is_some());
    }

    #[test]
    fn connection_limiter_simple_limit() {
        let limiter = ConnectionLimiter::new(2);
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let _g1 = limiter.try_acquire(ip).unwrap();
        let _g2 = limiter.try_acquire(ip).unwrap();
        // This third connection should not be allowed.
        assert!(
            limiter.try_acquire(ip).is_none(),
            "The connection should not be allowed"
        );
        drop(_g1);
        // Now this new connection should be allowed.
        assert!(
            limiter.try_acquire(ip).is_some(),
            "The connection should be allowed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connection_limiter_concurrent_access() {
        let limiter = Arc::new(ConnectionLimiter::new(10));
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let barrier = Arc::new(tokio::sync::Barrier::new(50));

        let handles: Vec<_> = (0..50)
            .map(|_| {
                let l = limiter.clone();
                let b = barrier.clone();
                tokio::spawn(async move {
                    b.wait().await; // wait for all threads
                    let _guard = l.try_acquire(ip);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    // _guard drop here.
                })
            })
            .collect();

        futures::future::join_all(handles).await;

        // Check for leaks.
        assert!(!limiter.connections.contains_key(&ip), "Connection leak");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connection_limiter_enforcement_concurrent() {
        let limiter = Arc::new(ConnectionLimiter::new(10));
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let barrier = Arc::new(tokio::sync::Barrier::new(50));

        let handles: Vec<_> = (0..50)
            .map(|_| {
                let l = limiter.clone();
                let b = barrier.clone();
                tokio::spawn(async move {
                    b.wait().await;
                    let guard = l.try_acquire(ip);
                    let success = guard.is_some();

                    if success {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    success
                })
            })
            .collect();

        let results = futures::future::join_all(handles).await;
        let total_success = results.into_iter().filter(|r| *r.as_ref().unwrap()).count();

        assert_eq!(
            total_success, 10,
            "The limit of 10 connections was not enforced: {total_success}"
        );
    }
}
