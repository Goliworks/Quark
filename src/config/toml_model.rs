use std::{collections::HashMap, net::SocketAddr};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    pub services: HashMap<String, Service>,
}

#[derive(Debug, Deserialize)]
pub struct Service {
    pub domain: String,
    pub location: String,
    pub locations: Option<Vec<Locations>>,
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
    pub kind: Option<String>,
}
