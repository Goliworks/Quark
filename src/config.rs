pub mod tls;
mod toml_model;
use argh::FromArgs;
use bincode::{Decode, Encode};
use hyper::StatusCode;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};
use toml_model::{ConfigToml, SubConfigToml};

use crate::utils::{self, extract_vars_from_string, generate_u32_id};

const MAIN_SERVER_NAME: &str = "main";
const DEFAULT_PORT: u16 = 80;
const DEFAULT_PORT_HTTPS: u16 = 443;
const DEFAULT_PROXY_TIMEOUT: u64 = 60;
const DEFAULT_TLS_REDIRECTION: bool = true;
const DEFAULT_REDIRECTION_CODE: u16 = 301; // Permanent.
const DEFAULT_BACKLOG: i32 = 4096;
const DEFAULT_MAX_CONNECTIONS: usize = 1024;
const DEFAULT_MAX_REQUESTS: usize = 100;

const DEFAULT_CONFIG_FILE_PATH: &str = "/etc/quark/config.toml";
const DEFAULT_LOG_PATH: &str = "/var/log/quark";

#[derive(Debug, Clone, Encode, Decode)]
pub struct ServiceConfig {
    pub servers: HashMap<String, Server>, // name -> Server
    pub global: Global,
    pub empty: bool,
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
    pub port: u16,
    pub https_port: u16,
    pub tls: Option<Vec<TlsCertificate>>,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct ServerParams {
    pub targets: BTreeMap<String, TargetType>, // Domain -> Location
    pub auto_tls: Option<Vec<String>>,
    pub proxy_timeout: u64,
}
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct TlsCertificate {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Locations {
    pub id: u32,
    pub locations: Vec<String>,
    pub strict_uri: bool, // default false. Used to check if the path must be conserved in the redirection.
    pub algo: Option<String>,
    pub weights: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct FileServer {
    pub location: String,
    pub strict_uri: bool, // default false. Used to check if the path must be conserved in the redirection.
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Redirection {
    pub location: String,
    pub strict_uri: bool, // default false. Used to check if the path must be conserved in the redirection.
    pub code: u16,
}

#[derive(Debug, Clone, Encode, Decode)]
pub enum TargetType {
    Location(Locations),
    FileServer(FileServer),
    Redirection(Redirection),
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

        // Check if the toml config has services.
        // If not, define the ServiceConfig as empty
        // to serve the Welcome page.
        let empty = config.services.is_none();

        let mut servers: HashMap<String, Server> = HashMap::new();

        // Declare all servers defined in the config.
        for (name, server) in &config.servers.unwrap_or(HashMap::new()) {
            let port = server.port.unwrap_or(DEFAULT_PORT);
            let https_port = server.https_port.unwrap_or(DEFAULT_PORT_HTTPS);
            let server = Server {
                params: ServerParams {
                    targets: BTreeMap::new(),
                    auto_tls: None,
                    proxy_timeout: server.proxy_timeout.unwrap_or(DEFAULT_PROXY_TIMEOUT),
                },
                port,
                https_port,
                tls: None,
            };
            servers.insert(name.clone(), server);
        }

        // Declare the main server if not declared.
        if !servers.contains_key(MAIN_SERVER_NAME) {
            let server = Server {
                params: ServerParams {
                    targets: BTreeMap::new(),
                    auto_tls: None,
                    proxy_timeout: DEFAULT_PROXY_TIMEOUT,
                },
                port: DEFAULT_PORT,
                https_port: DEFAULT_PORT_HTTPS,
                tls: None,
            };
            servers.insert(MAIN_SERVER_NAME.to_string(), server);
        }

        let services = config.services.unwrap_or(HashMap::new());
        for (_, service) in &services {
            // if service has TLS configuration, create a server for https.

            let mut tls_redirection = false;
            let server_name = service
                .server
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or(MAIN_SERVER_NAME);

            let server = servers.get_mut(server_name).unwrap();

            let port = server.port;
            let https_port = server.https_port;

            if let Some(tls) = &service.tls {
                let tls_cert = TlsCertificate {
                    cert: tls.certificate.clone(),
                    key: tls.key.clone(),
                };
                server.tls = Some(Vec::new());
                if let Some(tls) = &mut server.tls {
                    if !tls.contains(&tls_cert) {
                        // Add the certificate to the list.
                        tls.push(tls_cert);
                    }
                }
                tls_redirection = tls.redirection.unwrap_or(DEFAULT_TLS_REDIRECTION);
            }

            manage_locations_and_redirections(server, service, &config.loadbalancers);
            www_auto_redirection(
                server,
                service,
                if service.tls.is_some() {
                    https_port.clone()
                } else {
                    port
                },
                service.tls.is_some() && tls_redirection,
            );

            // Define if a tls redirection should be done.
            if tls_redirection {
                let domain = service.domain.clone();
                let tls_port = https_port.clone();
                let tls_domain = if tls_port != DEFAULT_PORT_HTTPS {
                    format!("{}:{}", domain, tls_port)
                } else {
                    domain
                };

                server
                    .params
                    .auto_tls
                    .get_or_insert_with(Vec::new)
                    .push(tls_domain);
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

        ServiceConfig {
            servers,
            global,
            empty,
        }
    }
}

fn get_toml_config(path: String) -> ConfigToml {
    println!("Loading config from {}", path);
    let toml_str = fs::read_to_string(&path).unwrap();
    let mut config: ConfigToml = toml::from_str(&toml_str).unwrap_or_else(|_| {
        panic!("Failed to parse toml file.\nInvalid configuration file.");
    });
    // import subconfiguration.
    if let Some(subconf) = &config.import {
        let mut conf_path = PathBuf::from(path);
        conf_path.pop();
        for file in subconf.iter() {
            let sub_config = import_sub_toml_config(file, conf_path.to_str().unwrap());
            // insert the subconfig into the main config.
            if let Some(services) = sub_config.services {
                config
                    .services
                    .get_or_insert_with(HashMap::new)
                    .extend(services);
            }
            if let Some(loadbalancers) = sub_config.loadbalancer {
                config
                    .loadbalancers
                    .get_or_insert_with(HashMap::new)
                    .extend(loadbalancers);
            }
        }
    }
    config
}

fn import_sub_toml_config(path: &str, dir: &str) -> SubConfigToml {
    let file_path = Path::new(path);
    let real_path = if file_path.is_relative() {
        Path::new(dir).join(file_path)
    } else {
        PathBuf::from(path)
    };
    let real_path = real_path.to_str().unwrap();
    let toml_str = fs::read_to_string(&real_path).unwrap();
    let config: SubConfigToml = toml::from_str(&toml_str).unwrap_or_else(|_| {
        panic!("Failed to parse toml file.\nInvalid configuration file.");
    });
    config
}

fn manage_locations_and_redirections(
    server: &mut Server,
    service: &toml_model::Service,
    loadbalancers: &Option<HashMap<String, toml_model::Loadbalancer>>,
) {
    // Other locations
    if let Some(locations) = &service.locations {
        for location in locations {
            // Remove last slash.
            let (source, strict_mode) = source_and_strict_mode(&location.source);
            // Get all backends info required for load balancing.
            let (backends, algo, weight) = get_backends_config(&location.target, loadbalancers);
            server.params.targets.insert(
                format!("{}{}", service.domain.clone(), source),
                TargetType::Location(Locations {
                    id: generate_u32_id(),
                    locations: backends,
                    strict_uri: strict_mode,
                    algo,
                    weights: weight,
                }),
            );
        }
    }
    if let Some(file_server) = &service.file_servers {
        for fs in file_server {
            let (source, strict_mode) = source_and_strict_mode(&fs.source);
            server.params.targets.insert(
                format!("{}{}", service.domain.clone(), source),
                TargetType::FileServer(FileServer {
                    location: fs.target.clone(),
                    strict_uri: strict_mode,
                }),
            );
        }
    }
    // Redirections.
    if let Some(redirections) = &service.redirections {
        for red in redirections {
            // Remove last slash.
            let (source, strict_mode) = source_and_strict_mode(&red.source);
            server.params.targets.insert(
                format!("{}{}", service.domain.clone(), source),
                TargetType::Redirection(Redirection {
                    location: red.target.clone(),
                    strict_uri: strict_mode,
                    code: match red.code {
                        // Available redirection codes.
                        Some(code @ (301 | 302 | 307 | 308)) => code,
                        _ => DEFAULT_REDIRECTION_CODE,
                    },
                }),
            );
        }
    }
}

fn get_backends_config(
    target: &str,
    loadbalancers: &Option<HashMap<String, toml_model::Loadbalancer>>,
) -> (Vec<String>, Option<String>, Option<Vec<u32>>) {
    let keys = extract_vars_from_string(target);
    let mut server_list: Vec<String> = Vec::new();
    let mut algo: Option<String> = None;
    let mut weight: Option<Vec<u32>> = None;

    // Only get the first key since you can only have one loadbalancer list.
    if let Some(key) = keys.get(0) {
        if let Some(loadbalancer) = loadbalancers.as_ref().unwrap().get(key) {
            let mut i = 0;
            let srv_nbr = loadbalancer.backends.len();
            for lb_server in &loadbalancer.backends {
                let server = if let Some(server) = server_list.get(i) {
                    server
                } else {
                    target
                };

                let server_url = server.to_string();
                let var = format!("${{{}}}", key);
                let server = server_url.replace(&var, &lb_server);

                server_list.push(server.to_string());
                algo = Some(loadbalancer.algo.clone());
                weight = manage_weights(srv_nbr, &loadbalancer.weights);
                i += 1;
            }
        }
    } else {
        server_list.push(target.to_string());
    }

    (server_list, algo, weight)
}

// Add or remmove weights if necessary.
fn manage_weights(srv_nbr: usize, weights: &Option<Vec<u32>>) -> Option<Vec<u32>> {
    match weights {
        Some(weights) => {
            let mut new_weights: Vec<u32> = Vec::with_capacity(srv_nbr);
            for i in 0..srv_nbr {
                new_weights.push(*weights.get(i).unwrap_or(&1));
            }
            Some(new_weights)
        }
        None => None,
    }
}

fn www_auto_redirection(server: &mut Server, service: &toml_model::Service, port: u16, tls: bool) {
    let domain: String;
    let target_domain: String;
    let default_port = if tls {
        DEFAULT_PORT_HTTPS
    } else {
        DEFAULT_PORT
    };
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

    server.params.targets.insert(
        domain,
        TargetType::Redirection(Redirection {
            location: target,
            strict_uri: false,
            code: StatusCode::MOVED_PERMANENTLY.as_u16(),
        }),
    );
}

fn source_and_strict_mode(source: &str) -> (&str, bool) {
    if source.ends_with("/*") {
        (&source[..source.len() - 2], false)
    } else {
        (utils::remove_last_slash(source), true)
    }
}
