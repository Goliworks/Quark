use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    pub services: HashMap<String, Service>,
    pub global: Option<Global>,
}

// Global config.
#[derive(Debug, Deserialize)]
pub struct Global {
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
