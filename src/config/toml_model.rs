use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    // All fields are optional because a config file can be empty
    // when the server is installed for the first time. But this
    // field is still required for a fully functional server.
    pub import: Option<Vec<String>>,
    pub global: Option<Global>,
    pub servers: Option<HashMap<String, Server>>,
    pub services: Option<HashMap<String, Service>>,
    pub loadbalancers: Option<HashMap<String, Loadbalancer>>,
}

#[derive(Debug, Deserialize)]
pub struct SubConfigToml {
    pub services: Option<HashMap<String, Service>>,
    pub loadbalancer: Option<HashMap<String, Loadbalancer>>,
}

// Global config.
#[derive(Debug, Deserialize)]
pub struct Global {
    pub backlog: Option<i32>,
    pub max_connections: Option<usize>,
    pub max_requests: Option<usize>,
    pub keepalive: Option<bool>,
    pub keepalive_timeout: Option<u64>,
    pub keepalive_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Server {
    pub port: Option<u16>,
    pub https_port: Option<u16>,
    pub proxy_timeout: Option<u64>,
    pub headers: Option<Headers>,
}

#[derive(Debug, Deserialize)]
pub struct Service {
    pub domain: String,
    pub server: Option<String>,
    pub locations: Option<Vec<Locations>>,
    pub file_servers: Option<Vec<FileServers>>,
    pub redirections: Option<Vec<Redirections>>,
    pub tls: Option<Tls>,
    pub headers: Option<Headers>,
}

#[derive(Debug, Deserialize)]
pub struct Headers {
    pub locations: Option<HeaderType>,
    pub file_servers: Option<HeaderAction>,
}

#[derive(Debug, Deserialize)]
pub struct HeaderType {
    pub request: Option<HeaderAction>,
    pub response: Option<HeaderAction>,
}

#[derive(Debug, Deserialize)]
pub struct HeaderAction {
    pub set: Option<HashMap<String, String>>,
    pub del: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub certificate: String,
    pub key: String,
    pub redirection: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Locations {
    pub source: String,
    pub target: String,
    pub headers: Option<HeaderType>,
}

#[derive(Debug, Deserialize)]
pub struct FileServers {
    pub source: String,
    pub target: String,
    pub authorized_dirs: Option<Vec<String>>,
    pub custom_404: Option<String>,
    pub headers: Option<HeaderAction>,
}

#[derive(Debug, Deserialize)]
pub struct Redirections {
    pub source: String,
    pub target: String,
    pub code: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub struct Loadbalancer {
    pub algo: String,
    pub backends: Vec<String>,
    pub weights: Option<Vec<u32>>,
}
