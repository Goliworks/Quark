use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    pub services: HashMap<String, Service>,
}

#[derive(Debug, Deserialize)]
pub struct Service {
    pub domain: String,
    pub location: String,
    pub tls: Option<Tls>,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub certificate: String,
    pub key: String,
}
