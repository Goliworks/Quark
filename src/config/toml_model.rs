use std::{collections::HashMap, net::SocketAddr};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    pub services: HashMap<String, Service>,
}

#[derive(Debug, Deserialize)]
pub struct Service {
    pub domain: String,
    pub location: SocketAddr,
    pub port: Option<u16>,
    pub tls: Option<Tls>,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub certificate: String,
    pub key: String,
    pub port: Option<u16>,
    pub redirection: Option<bool>,
}
