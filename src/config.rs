mod toml_model;
use std::{collections::HashMap, fs, net::SocketAddr};
use toml_model::ConfigToml;

const DEFAULT_PORT: u16 = 80;
const DEFAULT_PORT_TLS: u16 = 443;

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub servers: HashMap<u16, Server>, // Port -> Server
}

#[derive(Debug, Clone)]
pub struct Server {
    pub targets: HashMap<String, SocketAddr>, // Domain -> Location
    pub tls: Option<Vec<TlsCertificate>>,
}

#[derive(Debug, Clone)]
pub struct TlsCertificate {
    pub cert: String,
    pub key: String,
}

impl ServiceConfig {
    pub fn build_from(path: String) -> ServiceConfig {
        let config = get_toml_config(path);

        let mut servers: HashMap<u16, Server> = HashMap::new();
        for (_, service) in config.services {
            let port = service.port.unwrap_or(DEFAULT_PORT);

            // if service has TLS configuration, create a server for https.
            match service.tls {
                Some(tls) => {
                    let port_tls = tls.port.unwrap_or(DEFAULT_PORT_TLS);
                    let server_tls = servers.entry(port_tls).or_insert(Server {
                        targets: HashMap::new(),
                        tls: Some(Vec::new()),
                    });

                    server_tls
                        .targets
                        .insert(service.domain.clone(), service.location.clone());
                    server_tls.tls.as_mut().unwrap().push(TlsCertificate {
                        cert: tls.certificate,
                        key: tls.key,
                    });
                }
                None => {}
            }

            // create a default server for http.
            let server = servers.entry(port).or_insert(Server {
                targets: HashMap::new(),
                tls: None,
            });

            server
                .targets
                .insert(service.domain.clone(), service.location.clone());
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
