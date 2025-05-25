pub mod tls;
mod toml_model;
use std::{collections::HashMap, fs, net::SocketAddr};
use toml_model::ConfigToml;

use crate::utils;

pub const DEFAULT_PORT: u16 = 80;
const DEFAULT_PORT_TLS: u16 = 443;
const DEFAULT_PROXY_TIMEOUT: u64 = 60;
const DEFAULT_TLS_REDIRECTION: bool = true;

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub servers: HashMap<u16, Server>, // Port -> Server
}

#[derive(Debug, Clone)]
pub struct Server {
    pub params: ServerParams,
    pub tls: Option<Vec<TlsCertificate>>,
}

#[derive(Debug, Clone)]
pub struct ServerParams {
    pub targets: HashMap<String, String>, // Domain -> Location
    pub auto_tls: Option<Vec<String>>,
    pub proxy_timeout: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TlsCertificate {
    pub cert: String,
    pub key: String,
}

impl ServiceConfig {
    pub fn build_from(path: String) -> ServiceConfig {
        let config = get_toml_config(path);

        let mut servers: HashMap<u16, Server> = HashMap::new();
        for (_, service) in &config.services {
            let port = service.port.unwrap_or(DEFAULT_PORT);

            // if service has TLS configuration, create a server for https.

            let mut tls_redirection = false;

            match &service.tls {
                Some(tls) => {
                    let port_tls = tls.port.unwrap_or(DEFAULT_PORT_TLS);
                    let server_tls = servers.entry(port_tls).or_insert(Server {
                        params: ServerParams {
                            targets: HashMap::new(),
                            auto_tls: None,
                            proxy_timeout: service.proxy_timeout.unwrap_or(DEFAULT_PROXY_TIMEOUT),
                        },
                        tls: Some(Vec::new()),
                    });

                    // server_tls
                    //     .params
                    //     .targets
                    //     .insert(service.domain.clone(), service.location.clone());

                    // Other locations
                    if let Some(locations) = &service.locations {
                        for location in locations {
                            // Remove last /
                            let source = utils::remove_last_slash(&location.source);
                            server_tls.params.targets.insert(
                                format!("{}{}", service.domain.clone(), source),
                                location.target.clone(),
                            );
                        }
                    }

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
                    targets: HashMap::new(),
                    auto_tls: Some(Vec::new()),
                    proxy_timeout: service.proxy_timeout.unwrap_or(DEFAULT_PROXY_TIMEOUT),
                },
                tls: None,
            });

            // server
            //     .params
            //     .targets
            //     .insert(service.domain.clone(), service.location.clone());

            // Other locations
            if let Some(locations) = &service.locations {
                for location in locations {
                    let source = utils::remove_last_slash(&location.source);
                    server.params.targets.insert(
                        format!("{}{}", service.domain.clone(), source),
                        location.target.clone(),
                    );
                }
            }

            // Define if a tls redirection should be done.
            if tls_redirection {
                let domain = service.domain.clone();
                let port = service
                    .tls
                    .as_ref()
                    .unwrap()
                    .port
                    .unwrap_or(DEFAULT_PORT_TLS);

                let tls_domain = if port != DEFAULT_PORT_TLS {
                    format!("{}:{}", domain, port)
                } else {
                    domain
                };

                server.params.auto_tls.as_mut().unwrap().push(tls_domain);
            }
        }

        ServiceConfig { servers }
    }
}

pub fn get_toml_config(path: String) -> ConfigToml {
    let toml_str = fs::read_to_string(path).unwrap();
    let config: ConfigToml = toml::from_str(&toml_str).unwrap_or_else(|_| {
        panic!("Failed to parse toml file.\nInvalid configuration file.");
    });
    println!("{:?}", config);
    config
}
