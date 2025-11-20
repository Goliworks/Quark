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

use crate::{
    config::toml_model::{FileServers, Headers},
    utils::{self, extract_vars_from_string, generate_u32_id, get_path_and_file},
};

const MAIN_SERVER_NAME: &str = "main";
const DEFAULT_PORT: u16 = 80;
const DEFAULT_PORT_HTTPS: u16 = 443;
const DEFAULT_PROXY_TIMEOUT: u64 = 60;
const DEFAULT_TLS_REDIRECTION: bool = true;
const DEFAULT_REDIRECTION_CODE: u16 = 301; // Permanent.
const DEFAULT_BACKLOG: i32 = 4096;
const DEFAULT_MAX_CONNECTIONS: usize = 1024;
const DEFAULT_MAX_REQUESTS: usize = 100;
const DEFAULT_KEEPALIVE: bool = true;
const DEFAULT_KEEPALIVE_TIMEOUT: u64 = 60;
const DEFAULT_KEEPALIVE_INTERVAL: u64 = 20;
const DEFAULT_FORBIDDEN_DIR: bool = true;

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
    pub keepalive: bool,
    pub keepalive_timeout: u64,
    pub keepalive_interval: u64,
}

#[derive(Debug, Clone, Encode, Decode, Default)]
pub struct Server {
    pub params: ServerParams,
    pub port: u16,
    pub https_port: u16,
    pub tls: Option<Vec<TlsCertificate>>,
}

// Domain -> Location
type ServerParamsTargets = BTreeMap<String, TargetType>;

#[derive(Debug, Clone, Encode, Decode, Default)]
pub struct ServerParams {
    pub targets: ServerParamsTargets,
    pub strict_targets: ServerParamsTargets,
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
    pub params: TargetParams<Vec<String>>,
    pub algo: Option<String>,
    pub weights: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct FileServer {
    pub params: TargetParams<String>,
    pub fallback_file: Option<String>, // for 404 or spa page.
    pub is_fallback_404: bool,         // for 404 http status.
    pub forbidden_dir: bool,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Redirection {
    pub params: TargetParams<String>,
    pub code: u16,
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct TargetParams<T> {
    pub location: T,
    pub headers: ConfigHeaders,
}

#[derive(Debug, Clone, Encode, Decode, Default)]
pub struct ConfigHeaders {
    pub request: Option<ConfigHeadersActions>,
    pub response: Option<ConfigHeadersActions>,
}

#[derive(Debug, Clone, Encode, Decode, Default, PartialEq, Eq)]
pub struct ConfigHeadersActions {
    pub set: Option<HashMap<String, String>>,
    pub del: Option<Vec<String>>,
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
        if let Some(server_map) = &config.servers {
            for (name, server) in server_map {
                let port = server.port.unwrap_or(DEFAULT_PORT);
                let https_port = server.https_port.unwrap_or(DEFAULT_PORT_HTTPS);
                let server = Server {
                    params: ServerParams {
                        targets: BTreeMap::new(),
                        strict_targets: BTreeMap::new(),
                        auto_tls: None,
                        proxy_timeout: server.proxy_timeout.unwrap_or(DEFAULT_PROXY_TIMEOUT),
                    },
                    port,
                    https_port,
                    tls: None,
                };
                servers.insert(name.clone(), server);
            }
        }

        // Declare the main server if not declared.
        if !servers.contains_key(MAIN_SERVER_NAME) {
            let server = Server {
                params: ServerParams {
                    targets: BTreeMap::new(),
                    strict_targets: BTreeMap::new(),
                    auto_tls: None,
                    proxy_timeout: DEFAULT_PROXY_TIMEOUT,
                },
                port: DEFAULT_PORT,
                https_port: DEFAULT_PORT_HTTPS,
                tls: None,
            };
            servers.insert(MAIN_SERVER_NAME.to_string(), server);
        }

        let services = config.services.unwrap_or_default();
        for service in services.values() {
            // if service has TLS configuration, create a server for https.

            let mut tls_redirection = false;
            let server_name = service.server.as_deref().unwrap_or(MAIN_SERVER_NAME);

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

            let server_headers = config
                .servers
                .as_ref()
                .and_then(|servers| servers.get(server_name))
                .and_then(|server| server.headers.as_ref());

            manage_server_targets(server, service, &config.loadbalancers, server_headers);
            www_auto_redirection(
                &mut server.params.targets,
                &service.domain,
                if service.tls.is_some() {
                    https_port
                } else {
                    port
                },
                service.tls.is_some() && tls_redirection,
            );

            // Define if a tls redirection should be done.
            if tls_redirection {
                let domain = service.domain.clone();
                let tls_port = https_port;
                let tls_domain = if tls_port != DEFAULT_PORT_HTTPS {
                    format!("{domain}:{tls_port}")
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

        let global_config = config.global.as_ref();
        let global = Global {
            backlog: global_config
                .and_then(|g| g.backlog)
                .unwrap_or(DEFAULT_BACKLOG),
            max_conn: global_config
                .and_then(|g| g.max_connections)
                .unwrap_or(DEFAULT_MAX_CONNECTIONS),
            max_req: global_config
                .and_then(|g| g.max_requests)
                .unwrap_or(DEFAULT_MAX_REQUESTS),
            keepalive: global_config
                .and_then(|g| g.keepalive)
                .unwrap_or(DEFAULT_KEEPALIVE),
            keepalive_timeout: global_config
                .and_then(|g| g.keepalive_timeout)
                .unwrap_or(DEFAULT_KEEPALIVE_TIMEOUT),
            keepalive_interval: global_config
                .and_then(|g| g.keepalive_interval)
                .unwrap_or(DEFAULT_KEEPALIVE_INTERVAL),
        };

        ServiceConfig {
            servers,
            global,
            empty,
        }
    }
}

fn get_toml_config(path: String) -> ConfigToml {
    println!("Loading config from {path}");
    let toml_str = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("Failed to open toml file. {path} \n{e}");
        std::process::exit(1);
    });
    let mut config: ConfigToml = toml::from_str(&toml_str).unwrap_or_else(|e| {
        eprintln!("Failed to parse toml file.\nInvalid configuration file.\n{e}");
        std::process::exit(1);
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
    let toml_str = fs::read_to_string(real_path).unwrap_or_else(|e| {
        eprintln!("Failed to open toml file. {real_path} \n{e}");
        std::process::exit(1);
    });
    let config: SubConfigToml = toml::from_str(&toml_str).unwrap_or_else(|e| {
        eprintln!("Failed to parse toml file.\nInvalid configuration file.\n{e}");
        std::process::exit(1);
    });
    config
}

fn manage_server_targets(
    server: &mut Server,
    service: &toml_model::Service,
    loadbalancers: &Option<HashMap<String, toml_model::Loadbalancer>>,
    server_headers: Option<&Headers>,
) {
    // Manage headers
    let (l_headers, fs_headers) = headers::get_config_headers_from(server_headers);
    // Locations
    if let Some(locations) = &service.locations {
        // Manage locations.
        for location in locations {
            // Custom headers for this specific location.
            let mut headers = l_headers.clone();

            headers::apply_header_actions(
                service.headers.as_ref().and_then(|h| h.locations.as_ref()),
                &mut headers,
            );
            headers::apply_header_actions(location.headers.as_ref(), &mut headers);

            // Remove last slash.
            let (source, strict_mode) = source_and_strict_mode(&location.source);
            // Get all backends info required for load balancing.
            let (backends, algo, weight) = get_backends_config(&location.target, loadbalancers);

            let key = format!("{}{}", service.domain, source);
            let target = TargetType::Location(Locations {
                id: generate_u32_id(),
                params: TargetParams {
                    location: backends,
                    headers,
                },
                algo,
                weights: weight,
            });

            if strict_mode {
                server.params.strict_targets.insert(key, target);
            } else {
                server.params.targets.insert(key, target);
            }
        }
    }
    if let Some(file_server) = &service.file_servers {
        for fs in file_server {
            manage_file_servers(
                fs,
                service.domain.clone(),
                &mut server.params.targets,
                &mut server.params.strict_targets,
                &fs_headers,
                service.headers.as_ref(),
            );
        }
    }
    // Redirections.
    if let Some(redirections) = &service.redirections {
        // Manage redirections.
        for red in redirections {
            // Remove last slash.
            let (source, strict_mode) = source_and_strict_mode(&red.source);

            let key = format!("{}{}", service.domain, source);
            let target = TargetType::Redirection(Redirection {
                params: TargetParams {
                    location: red.target.clone(),
                    headers: ConfigHeaders::default(),
                },
                code: match red.code {
                    // Available redirection codes.
                    Some(code @ (301 | 302 | 307 | 308)) => code,
                    _ => DEFAULT_REDIRECTION_CODE,
                },
            });

            if strict_mode {
                server.params.strict_targets.insert(key, target);
            } else {
                server.params.targets.insert(key, target);
            }
        }
    }
}

fn manage_file_servers(
    fs: &FileServers,
    domain: String,
    targets: &mut ServerParamsTargets,
    strict_targets: &mut ServerParamsTargets,
    headers: &ConfigHeaders,
    service_headers: Option<&Headers>,
) {
    let (source, strict_mode) = source_and_strict_mode(&fs.source);
    let (target, file_name) = get_path_and_file(&fs.target);
    let target_str = target.to_string_lossy().to_string();
    let mut is_fallback_404 = false;

    let file_path = if file_name.is_some() {
        Some(fs.target.clone())
    } else if fs.custom_404.is_some() {
        is_fallback_404 = true;
        Some(fs.custom_404.as_ref().unwrap().clone())
    } else {
        None
    };

    // Custom headers for this specific file server.
    let mut headers = headers.clone();

    if let Some(service_header) = service_headers {
        if let Some(fsh) = &service_header.file_servers {
            headers::merge_headers_actions(fsh, &mut headers.response);
        }
    }

    if let Some(ha) = &fs.headers {
        headers::merge_headers_actions(ha, &mut headers.response);
    }

    let key = format!("{}{}", domain, source);
    let target = TargetType::FileServer(FileServer {
        params: TargetParams {
            location: target_str.clone(),
            headers: headers.clone(),
        },
        fallback_file: file_path.clone(),
        is_fallback_404,
        forbidden_dir: DEFAULT_FORBIDDEN_DIR,
    });

    if strict_mode {
        strict_targets.insert(key, target);
    } else {
        targets.insert(key, target);
    }

    if let Some(ads) = &fs.authorized_dirs {
        for ad in ads {
            let (dir, strict_mode, access) = dir_strict_mode_and_access(ad);
            let key = format!("{}{}{}", domain, source, dir);
            let target = TargetType::FileServer(FileServer {
                params: TargetParams {
                    location: format!("{}{}", target_str, dir),
                    headers: headers.clone(),
                },
                fallback_file: file_path.clone(),
                is_fallback_404,
                forbidden_dir: access,
            });

            if strict_mode {
                strict_targets.insert(key, target);
            } else {
                targets.insert(key, target);
            }
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
    if let Some(key) = keys.first() {
        if let Some(loadbalancer) = loadbalancers.as_ref().unwrap().get(key) {
            let srv_nbr = loadbalancer.backends.len();
            for (i, lb_server) in loadbalancer.backends.iter().enumerate() {
                let server = if let Some(server) = server_list.get(i) {
                    server
                } else {
                    target
                };

                let server_url = server.to_string();
                let var = format!("${{{key}}}");
                let server = server_url.replace(&var, lb_server);

                server_list.push(server.to_string());
                algo = Some(loadbalancer.algo.clone());
                weight = manage_weights(srv_nbr, &loadbalancer.weights);
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

fn www_auto_redirection(
    server_targets: &mut ServerParamsTargets,
    service_domain: &str,
    port: u16,
    tls: bool,
) {
    let domain: String;
    let target_domain: String;
    let default_port = if tls {
        DEFAULT_PORT_HTTPS
    } else {
        DEFAULT_PORT
    };
    // If the configured domain doesn't start with www, redirect every request
    // that starts with www to the configured domain.
    if !service_domain.starts_with("www.") {
        domain = format!("www.{}", service_domain);
        target_domain = service_domain.to_string();
    // Otherwise, redirect every request that doesn't start with www to www.domain.
    } else {
        domain = service_domain.strip_prefix("www.").unwrap().to_string();
        target_domain = service_domain.to_string();
    }
    let target = format!(
        "http{}://{}{}",
        if tls { "s" } else { "" },
        target_domain,
        if port != default_port {
            format!(":{port}")
        } else {
            "".to_string()
        }
    );

    server_targets.insert(
        domain,
        TargetType::Redirection(Redirection {
            params: TargetParams {
                location: target,
                headers: ConfigHeaders::default(),
            },
            code: StatusCode::MOVED_PERMANENTLY.as_u16(),
        }),
    );
}

fn dir_strict_mode_and_access(path: &str) -> (&str, bool, bool) {
    if let Some(p) = path.strip_prefix("!") {
        // forbidden directory.
        let (source, mode) = source_and_strict_mode(p);
        (source, mode, true)
    } else {
        let (source, mode) = source_and_strict_mode(path);
        (source, mode, false)
    }
}

fn source_and_strict_mode(source: &str) -> (&str, bool) {
    if let Some(s) = source.strip_suffix("/*") {
        (s, false)
    } else {
        (utils::remove_last_slash(source), true)
    }
}

mod headers {
    use crate::config::{
        toml_model::{HeaderAction, HeaderType, Headers},
        ConfigHeaders, ConfigHeadersActions,
    };

    pub fn get_config_headers_from(
        headers: Option<&Headers>,
    ) -> (
        ConfigHeaders, // Location
        ConfigHeaders, // FileServer
    ) {
        let mut l_headers = ConfigHeaders::default();
        let mut fs_headers = ConfigHeaders::default();
        if let Some(h) = headers {
            if let Some(locations) = &h.locations {
                if let Some(request) = &locations.request {
                    l_headers.request = Some(process_headers_set_del(request));
                }
                if let Some(response) = &locations.response {
                    l_headers.response = Some(process_headers_set_del(response));
                }
            }
            if let Some(response) = &h.file_servers {
                fs_headers.response = Some(process_headers_set_del(response));
            }
        }

        (l_headers, fs_headers)
    }

    fn process_headers_set_del(action: &HeaderAction) -> ConfigHeadersActions {
        let mut config_action = ConfigHeadersActions::default();
        if let Some(set) = &action.set {
            config_action.set = Some(set.clone());
        }
        if let Some(del) = &action.del {
            config_action.del = Some(del.clone());
        }
        config_action
    }

    pub fn apply_header_actions(
        header_type: Option<&HeaderType>,
        config_headers: &mut ConfigHeaders,
    ) {
        if let Some(ht) = header_type {
            // Request headers.
            if let Some(req) = &ht.request {
                merge_headers_actions(req, &mut config_headers.request);
            }
            // Response headers.
            if let Some(res) = &ht.response {
                merge_headers_actions(res, &mut config_headers.response);
            }
        }
    }

    pub fn merge_headers_actions(ha: &HeaderAction, cha: &mut Option<ConfigHeadersActions>) {
        let actions = process_headers_set_del(ha);
        let target = cha.get_or_insert_default();

        merge_option_collections(&mut target.set, actions.set);
        merge_option_collections(&mut target.del, actions.del);
    }

    fn merge_option_collections<T>(target: &mut Option<T>, source: Option<T>)
    where
        T: Extend<T::Item> + IntoIterator,
    {
        match (target.as_mut(), source) {
            (Some(t), Some(s)) => {
                t.extend(s);
            }
            (None, Some(s)) => *target = Some(s),
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::toml_model::HeaderAction;

    use super::*;

    fn header_action_mock() -> HeaderAction {
        HeaderAction {
            set: Some(HashMap::from([
                ("set1".to_string(), "ha1".to_string()),
                ("set2".to_string(), "ha2".to_string()),
            ])),
            del: Some(vec!["del1".to_string(), "del2".to_string()]),
        }
    }

    fn server_mock() -> Server {
        Server {
            params: ServerParams {
                targets: BTreeMap::new(),
                strict_targets: BTreeMap::new(),
                auto_tls: None,
                proxy_timeout: DEFAULT_PROXY_TIMEOUT,
            },
            port: DEFAULT_PORT,
            https_port: DEFAULT_PORT_HTTPS,
            tls: None,
        }
    }

    fn assert_www_redirection(
        source_domain: &str,
        target_domain: &str,
        expected_url: &str,
        port: u16,
        tls: bool,
    ) {
        let mut server = server_mock();
        www_auto_redirection(&mut server.params.targets, target_domain, port, tls);
        let target = server.params.targets.get(source_domain).unwrap();

        assert!(
            matches!(target, TargetType::Redirection(_)),
            "Expected TargetType::Redirection"
        );

        if let TargetType::Redirection(url) = target {
            assert_eq!(url.params.location, expected_url);
        }
    }

    #[test]
    fn merge_headers_actions() {
        let ha = header_action_mock();
        let mut cha = Some(ConfigHeadersActions {
            set: Some(HashMap::from([
                ("set2".to_string(), "cha1".to_string()),
                ("set3".to_string(), "cha2".to_string()),
            ])),
            del: Some(vec!["del3".to_string()]),
        });
        headers::merge_headers_actions(&ha, &mut cha);
        cha.as_mut().unwrap().del.as_mut().unwrap().sort();
        let expected = Some(ConfigHeadersActions {
            set: Some(HashMap::from([
                ("set1".to_string(), "ha1".to_string()),
                ("set2".to_string(), "ha2".to_string()),
                ("set3".to_string(), "cha2".to_string()),
            ])),
            del: Some(vec![
                "del1".to_string(),
                "del2".to_string(),
                "del3".to_string(),
            ]),
        });
        assert_eq!(cha, expected);
    }

    #[test]
    fn merge_headers_actions_ha_empty() {
        let ha = header_action_mock();
        let mut cha = None;
        headers::merge_headers_actions(&ha, &mut cha);
        let expected = Some(ConfigHeadersActions {
            set: Some(HashMap::from([
                ("set1".to_string(), "ha1".to_string()),
                ("set2".to_string(), "ha2".to_string()),
            ])),
            del: Some(vec!["del1".to_string(), "del2".to_string()]),
        });
        assert_eq!(cha, expected);
    }

    #[test]
    fn merge_headers_actions_cha_empty() {
        let ha = HeaderAction {
            set: None,
            del: None,
        };
        let mut cha = Some(ConfigHeadersActions {
            set: Some(HashMap::from([
                ("set1".to_string(), "cha1".to_string()),
                ("set2".to_string(), "cha2".to_string()),
            ])),
            del: Some(vec!["del1".to_string()]),
        });
        headers::merge_headers_actions(&ha, &mut cha);
        let expected = Some(ConfigHeadersActions {
            set: Some(HashMap::from([
                ("set1".to_string(), "cha1".to_string()),
                ("set2".to_string(), "cha2".to_string()),
            ])),
            del: Some(vec!["del1".to_string()]),
        });
        assert_eq!(cha, expected);
    }

    #[test]
    fn www_subdomain_to_apex_domain_http() {
        assert_www_redirection(
            "www.example.com",
            "example.com",
            "http://example.com",
            DEFAULT_PORT,
            false,
        );
    }

    #[test]
    fn www_subdomain_to_apex_domain_https() {
        assert_www_redirection(
            "www.example.com",
            "example.com",
            "https://example.com",
            DEFAULT_PORT_HTTPS,
            true,
        );
    }

    #[test]
    fn apex_domain_to_www_subdomain_http() {
        assert_www_redirection(
            "example.com",
            "www.example.com",
            "http://www.example.com",
            DEFAULT_PORT,
            false,
        );
    }

    #[test]
    fn apex_domain_to_www_subdomain_https() {
        assert_www_redirection(
            "example.com",
            "www.example.com",
            "https://www.example.com",
            DEFAULT_PORT_HTTPS,
            true,
        );
    }

    #[test]
    fn www_subdomain_to_apex_domain_http_with_port() {
        assert_www_redirection(
            "www.example.com",
            "example.com",
            "http://example.com:8080",
            8080,
            false,
        );
    }

    #[test]
    fn www_subdomain_to_apex_domain_https_with_port() {
        assert_www_redirection(
            "www.example.com",
            "example.com",
            "https://example.com:8443",
            8443,
            true,
        );
    }
}
