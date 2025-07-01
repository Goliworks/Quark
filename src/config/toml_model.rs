use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    // services is optionnal because a config file can be empty
    // when the server is installed for the first time. But this
    // field is still required for a fully functional server.
    pub global: Option<Global>,
    pub services: Option<HashMap<String, Service>>,
    pub loadbalancer: Option<HashMap<String, Loadbalancer>>,
}

// Global config.
#[derive(Debug, Deserialize)]
pub struct Global {
    pub backlog: Option<i32>,
    pub max_connections: Option<usize>,
    pub max_requests: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct Service {
    pub domain: String,
    pub locations: Option<Vec<Locations>>,
    pub redirections: Option<Vec<Redirections>>,
    pub port: Option<u16>,
    pub tls: Option<Tls>,
    pub proxy_timeout: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub certificate: String,
    pub key: String,
    pub port: Option<u16>,
    pub redirection: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Locations {
    pub source: String,
    pub target: String,
    pub serve_files: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Redirections {
    pub source: String,
    pub target: String,
    pub temporary: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Loadbalancer {
    pub algo: String,
    pub servers: Vec<String>,
    pub weights: Option<Vec<u32>>,
}
