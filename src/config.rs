pub mod tls;
mod toml_model;
use argh::FromArgs;
use bincode::{Decode, Encode};
use hyper::StatusCode;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
};
use toml_model::ConfigToml;

use crate::utils;

const DEFAULT_PORT: u16 = 80;
const DEFAULT_PORT_TLS: u16 = 443;
const DEFAULT_PROXY_TIMEOUT: u64 = 60;
const DEFAULT_TLS_REDIRECTION: bool = true;
const DEFAULT_TEMPORARY_REDIRECT: bool = false;
const DEFAULT_SERVE_FILES: bool = false;
const DEFAULT_BACKLOG: i32 = 4096;
const DEFAULT_MAX_CONNECTIONS: usize = 1024;
const DEFAULT_MAX_REQUESTS: usize = 100;

const DEFAULT_CONFIG_FILE_PATH: &str = "/etc/quark/config.toml";
const DEFAULT_LOG_PATH: &str = "/var/log/quark";

#[derive(Debug, Clone, Encode, Decode)]
pub struct ServiceConfig {
    pub servers: HashMap<u16, Server>, // Port -> Server
    pub global: Global,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Global {
    pub backlog: i32,
    pub max_conn: usize,
    pub max_req: usize,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Server {
    pub params: ServerParams,
    pub tls: Option<Vec<TlsCertificate>>,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct ServerParams {
    pub targets: BTreeMap<String, Target>, // Domain -> Location
    pub redirections: BTreeMap<String, Redirection>, // Domain -> redirection
    pub auto_tls: Option<Vec<String>>,
    pub proxy_timeout: u64,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct TlsCertificate {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Target {
    pub location: String,
    pub strict_uri: bool, // default false. Used to check if the path must be conserved in the redirection.
    pub serve_files: bool,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Redirection {
    pub location: String,
    pub strict_uri: bool, // default false. Used to check if the path must be conserved in the redirection.
    pub code: u16,
}

#[derive(FromArgs)]
#[argh(description = "certificates")]
pub struct Options {
    /// config file path.
    #[argh(option, short = 'c', default = "DEFAULT_CONFIG_FILE_PATH.to_string()")]
    pub config: String,
    /// logs directory path
    #[argh(option, short = 'l', default = "DEFAULT_LOG_PATH.to_string()")]
    pub logs: String,

    /// run as child process
    #[argh(switch)]
    _child_process: bool,
}

impl ServiceConfig {
    pub fn build_from(path: String) -> ServiceConfig {
        let config = get_toml_config(path);

        let mut servers: HashMap<u16, Server> = HashMap::new();
        let services = config.services.unwrap_or(HashMap::new());
        for (_, service) in &services {
            let port = service.port.unwrap_or(DEFAULT_PORT);

            // if service has TLS configuration, create a server for https.

            let mut tls_redirection = false;

            match &service.tls {
                Some(tls) => {
                    let port_tls = tls.port.unwrap_or(DEFAULT_PORT_TLS);
                    let server_tls = servers.entry(port_tls).or_insert(Server {
                        params: ServerParams {
                            targets: BTreeMap::new(),
                            redirections: BTreeMap::new(),
                            auto_tls: None,
                            proxy_timeout: service.proxy_timeout.unwrap_or(DEFAULT_PROXY_TIMEOUT),
                        },
                        tls: Some(Vec::new()),
                    });

                    manage_locations_and_redirections(server_tls, service);
                    www_auto_redirection(server_tls, service, port_tls, true);
                    // Create a struct with the found certificates.
                    let tls_cert = TlsCertificate {
                        cert: tls.certificate.clone(),
                        key: tls.key.clone(),
                    };

                    // Check if the certificate is already in the list.
                    if let Some(tls) = &mut server_tls.tls {
                        if !tls.contains(&tls_cert) {
                            // Add the certificate to the list.
                            tls.push(tls_cert);
                        }
                    }
                    tls_redirection = tls.redirection.unwrap_or(DEFAULT_TLS_REDIRECTION);
                }
                None => {}
            }

            // Create a default server for http.
            let server = servers.entry(port).or_insert(Server {
                params: ServerParams {
                    targets: BTreeMap::new(),
                    redirections: BTreeMap::new(),
                    auto_tls: Some(Vec::new()),
                    proxy_timeout: service.proxy_timeout.unwrap_or(DEFAULT_PROXY_TIMEOUT),
                },
                tls: None,
            });

            manage_locations_and_redirections(server, service);
            www_auto_redirection(
                server,
                service,
                if service.tls.is_some() {
                    service
                        .tls
                        .as_ref()
                        .unwrap()
                        .port
                        .unwrap_or(DEFAULT_PORT_TLS)
                } else {
                    port
                },
                service.tls.is_some() && tls_redirection,
            );

            // Define if a tls redirection should be done.
            if tls_redirection {
                let domain = service.domain.clone();
                let tls_port = service
                    .tls
                    .as_ref()
                    .unwrap()
                    .port
                    .unwrap_or(DEFAULT_PORT_TLS);

                let tls_domain = if tls_port != DEFAULT_PORT_TLS {
                    format!("{}:{}", domain, tls_port)
                } else {
                    domain
                };

                server.params.auto_tls.as_mut().unwrap().push(tls_domain);
            }
        }

        let global = Global {
            backlog: config
                .global
                .as_ref()
                .and_then(|g| g.backlog)
                .unwrap_or(DEFAULT_BACKLOG),
            max_conn: config
                .global
                .as_ref()
                .and_then(|g| g.max_connections)
                .unwrap_or(DEFAULT_MAX_CONNECTIONS),
            max_req: config
                .global
                .as_ref()
                .and_then(|g| g.max_requests)
                .unwrap_or(DEFAULT_MAX_REQUESTS),
        };

        ServiceConfig { servers, global }
    }
}

fn get_toml_config(path: String) -> ConfigToml {
    let toml_str = fs::read_to_string(path).unwrap();
    let config: ConfigToml = toml::from_str(&toml_str).unwrap_or_else(|_| {
        panic!("Failed to parse toml file.\nInvalid configuration file.");
    });
    config
}

fn manage_locations_and_redirections(server: &mut Server, service: &toml_model::Service) {
    // Other locations
    if let Some(locations) = &service.locations {
        for location in locations {
            // Remove last /
            let (source, strict_mode) = source_and_strict_mode(&location.source);
            server.params.targets.insert(
                format!("{}{}", service.domain.clone(), source),
                Target {
                    location: location.target.clone(),
                    strict_uri: strict_mode,
                    serve_files: location.serve_files.unwrap_or(DEFAULT_SERVE_FILES),
                },
            );
        }
    }
    // Redirections.
    if let Some(redirections) = &service.redirections {
        for red in redirections {
            // Remove last /
            let (source, strict_mode) = source_and_strict_mode(&red.source);
            server.params.redirections.insert(
                format!("{}{}", service.domain.clone(), source),
                Redirection {
                    location: red.target.clone(),
                    strict_uri: strict_mode,
                    code: if red.temporary.unwrap_or(DEFAULT_TEMPORARY_REDIRECT) {
                        StatusCode::TEMPORARY_REDIRECT.as_u16()
                    } else {
                        StatusCode::PERMANENT_REDIRECT.as_u16()
                    },
                },
            );
        }
    }
}

fn www_auto_redirection(server: &mut Server, service: &toml_model::Service, port: u16, tls: bool) {
    let domain: String;
    let target_domain: String;
    let default_port = if tls { DEFAULT_PORT_TLS } else { DEFAULT_PORT };
    // If the configured domain doesn't start with www, redirect every request
    // that starts with www to the configured domain.
    if !service.domain.starts_with("www") {
        domain = format!("www.{}", service.domain);
        target_domain = service.domain.clone();
    // Otherwise, redirect every request that doesn't start with www to www.domain.
    } else {
        domain = service.domain.clone();
        target_domain = service.domain.strip_prefix("www.").unwrap().to_string();
    }
    let target = format!(
        "http{}://{}{}",
        if tls { "s" } else { "" },
        target_domain,
        if port != default_port {
            format!(":{}", port)
        } else {
            "".to_string()
        }
    );

    server.params.redirections.insert(
        domain,
        Redirection {
            location: target,
            strict_uri: false,
            code: StatusCode::MOVED_PERMANENTLY.as_u16(),
        },
    );
}

fn source_and_strict_mode(source: &str) -> (&str, bool) {
    if source.ends_with("/*") {
        (&source[..source.len() - 2], false)
    } else {
        (utils::remove_last_slash(source), true)
    }
}
