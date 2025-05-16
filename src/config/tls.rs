use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::sync::Arc;

use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::server::ResolvesServerCertUsingSni;
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
        let mut resolver = ResolvesServerCertUsingSni::new(); // println!("{:?}", x509_cert);

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

    fn add_certificate_to_resolver(
        &self,
        cert: &TlsCertificate,
        resolver: &mut ResolvesServerCertUsingSni,
    ) {
        let cert_file = &mut BufReader::new(File::open(&cert.cert).unwrap());
        let cert_buffer = cert_file.fill_buf().unwrap();

        let cert_der = load_certs(&cert.cert).unwrap();
        let key = load_private_key(&cert.key).unwrap();

        let key_sign = any_supported_type(&key).unwrap();

        let ck = CertifiedKey::new(cert_der, key_sign);

        let (_, pem) = parse_x509_pem(cert_buffer).unwrap();

        let domains: Vec<String> = match parse_x509_certificate(&pem.contents) {
            Ok((_, x509_cert)) => self.extract_domains_from_x509(&x509_cert),
            Err(e) => panic!("{:?}", e),
        };

        domains.iter().for_each(|domain| {
            println!("Domain: {}", domain);
            resolver.add(domain, ck.clone()).unwrap();
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
                                // avoid wildcard certificates.
                                if !dnsn.starts_with("*.") {
                                    domain_names.push(dnsn.to_string());
                                }
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
