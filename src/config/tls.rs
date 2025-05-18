use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::sync::Arc;

use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use x509_parser::parse_x509_certificate;
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::{GeneralName, ParsedExtension, X509Certificate};

use super::TlsCertificate;

pub struct TlsConfig<'a> {
    certs: &'a Vec<TlsCertificate>,
}

impl<'a> TlsConfig<'a> {
    pub fn new(certs: &'a Vec<TlsCertificate>) -> TlsConfig<'a> {
        TlsConfig { certs }
    }

    pub fn get_tls_config(&self) -> ServerConfig {
        let mut resolver = SniCertResolver::new();

        for cert in self.certs.iter() {
            println!("{}", cert.cert);
            self.add_certificate_to_resolver(cert, &mut resolver);
        }

        let mut config_tls = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));

        config_tls.alpn_protocols =
            vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

        config_tls
    }

    fn add_certificate_to_resolver(&self, cert: &TlsCertificate, resolver: &mut SniCertResolver) {
        let cert_der = load_certs(&cert.cert).unwrap();
        let cert_buffer = load_cert_buffer(&cert.cert);
        let key = load_private_key(&cert.key).unwrap();

        let key_sign = any_supported_type(&key).unwrap();

        let ck = CertifiedKey::new(cert_der, key_sign);

        let (_, pem) = parse_x509_pem(&cert_buffer).unwrap();

        let domains: Vec<String> = match parse_x509_certificate(&pem.contents) {
            Ok((_, x509_cert)) => self.extract_domains_from_x509(&x509_cert),
            Err(e) => panic!("{:?}", e),
        };

        domains.iter().for_each(|domain| {
            println!("Domain: {}", domain);
            resolver.add(domain, ck.clone());
        })
    }

    fn extract_domains_from_x509(&self, x509: &X509Certificate) -> Vec<String> {
        let mut domain_names: Vec<String> = Vec::new();
        for ext in x509.extensions() {
            match ext.parsed_extension() {
                ParsedExtension::SubjectAlternativeName(san) => {
                    for name in &san.general_names {
                        match name {
                            GeneralName::DNSName(dnsn) => {
                                println!("{}", dnsn);
                                domain_names.push(dnsn.to_string());
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        domain_names
    }
}

// Custom SNI resolver.
#[derive(Debug)]
struct SniCertResolver {
    certs: HashMap<String, Arc<CertifiedKey>>,
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        if let Some(server_name) = client_hello.server_name() {
            println!("SNI requested: {}", server_name);

            if let Some(cert) = self.certs.get(&server_name.to_string()) {
                println!("SNI resolved to: {}", server_name);
                return Some(cert.clone());
            }

            //  Try wildcards.
            let wildcard_name = convert_to_wildcard(&server_name);
            if let Some(cert) = self.certs.get(&wildcard_name) {
                println!("SNI resolved to: {}", wildcard_name);
                return Some(cert.clone());
            }
        }
        println!("No SNI provided by client.");
        None
    }
}

impl SniCertResolver {
    fn new() -> SniCertResolver {
        SniCertResolver {
            certs: HashMap::new(),
        }
    }

    fn add(&mut self, domain: &str, ck: CertifiedKey) {
        self.certs.insert(domain.to_string(), Arc::new(ck));
    }
}

fn convert_to_wildcard(server_name: &str) -> String {
    let explode_name: Vec<&str> = server_name.split('.').collect();
    let mut i: u8 = 0;
    let wildcard_name: Vec<&str> = explode_name
        .into_iter()
        .map(|x| {
            i += 1;
            if i == 1 {
                return "*";
            } else {
                return x;
            }
        })
        .collect();

    wildcard_name.join(".")
}

// Load certificates and keys from files.

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

// Load public certificate from file.
fn load_certs(filename: &str) -> io::Result<Vec<CertificateDer<'static>>> {
    // Open certificate file.
    let certfile =
        File::open(filename).map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = BufReader::new(certfile);

    // Load and return certificate.
    rustls_pemfile::certs(&mut reader).collect()
}

// Load private key from file.
fn load_private_key(filename: &str) -> io::Result<PrivateKeyDer<'static>> {
    // Open keyfile.
    let keyfile =
        File::open(filename).map_err(|e| error(format!("failed to open {}: {}", filename, e)))?;
    let mut reader = BufReader::new(keyfile);

    // Load and return a single private key.
    rustls_pemfile::private_key(&mut reader).map(|key| key.unwrap())
}

fn load_cert_buffer(filename: &str) -> Vec<u8> {
    let certfile = File::open(filename).unwrap();
    let mut reader = BufReader::new(certfile);
    let buffer = reader.fill_buf().unwrap();

    buffer.to_vec()
}
