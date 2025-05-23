use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use futures::{SinkExt, StreamExt};
use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, Watcher};
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use x509_parser::parse_x509_certificate;
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::{GeneralName, ParsedExtension, X509Certificate};

use futures::channel::mpsc::channel;

use super::TlsCertificate;

pub type CertifiedKeyList = HashMap<String, ArcSwap<CertifiedKey>>;

pub struct TlsConfig {
    certs: Vec<TlsCertificate>,
    paths_to_watch: Vec<PathBuf>,
}

impl TlsConfig {
    pub fn new(certs: Vec<TlsCertificate>) -> TlsConfig {
        TlsConfig {
            certs,
            paths_to_watch: Vec::new(),
        }
    }

    pub fn get_certified_key_list(&mut self) -> CertifiedKeyList {
        let mut ck_list: CertifiedKeyList = HashMap::new();

        for cert in self.certs.iter() {
            let path = Path::new(&cert.cert);
            let directory = path.parent().unwrap();
            let pathbuf = directory.to_path_buf();
            if !self.paths_to_watch.contains(&pathbuf) {
                self.paths_to_watch.push(pathbuf);
            }

            self.add_certificate_to_certified_key_list(cert, &mut ck_list);
        }

        ck_list
    }

    // Generate and return the rustls server config.
    pub fn get_tls_config(&self, resolver: SniCertResolver) -> ServerConfig {
        let mut config_tls = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));

        config_tls.alpn_protocols =
            vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

        config_tls
    }

    // Start to watch for certificates changes.
    // Run it in a separate task.
    pub async fn watch_certs(&self, ck_list: Arc<CertifiedKeyList>) {
        println!("Paths to watch: {:?}\n", self.paths_to_watch);

        let (mut tx, mut rx) = channel(1);

        let mut watcher = RecommendedWatcher::new(
            move |res| futures::executor::block_on(async { tx.send(res).await.unwrap() }),
            notify::Config::default(),
        )
        .unwrap();

        for path in &self.paths_to_watch {
            watcher
                .watch(path, notify::RecursiveMode::Recursive)
                .unwrap();
        }

        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    // println!("changed: {:?}", event);
                    if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write)) {
                        println!("File changed: {}", event.paths[0].display());

                        for cert in self.certs.iter() {
                            self.reload_certificates(cert, &ck_list);
                        }
                    }
                }

                Err(e) => println!("watch error: {:?}", e),
            }
        }
    }

    fn add_certificate_to_certified_key_list(
        &self,
        cert: &TlsCertificate,
        ck_list: &mut CertifiedKeyList,
    ) {
        let (domains, ck) = self.get_domains_and_ck(cert);

        domains.iter().for_each(|domain| {
            ck_list.insert(domain.to_string(), ArcSwap::new(ck.clone()));
        })
    }

    fn reload_certificates(&self, cert: &TlsCertificate, ck_list: &CertifiedKeyList) {
        let (domains, ck) = self.get_domains_and_ck(cert);

        domains.iter().for_each(|domain| {
            if let Some(ack) = ck_list.get(domain) {
                ack.store(ck.clone());
            }
        });
    }

    fn get_domains_and_ck(&self, cert: &TlsCertificate) -> (Vec<String>, Arc<CertifiedKey>) {
        let cert_der = load_certs(&cert.cert).unwrap();
        let cert_buffer = load_cert_buffer(&cert.cert);
        let key = load_private_key(&cert.key).unwrap();

        let key_sign = any_supported_type(&key).unwrap();

        let ck = Arc::new(CertifiedKey::new(cert_der, key_sign));

        let (_, pem) = parse_x509_pem(&cert_buffer).unwrap();

        let domains: Vec<String> = match parse_x509_certificate(&pem.contents) {
            Ok((_, x509_cert)) => self.extract_domains_from_x509(&x509_cert),
            Err(e) => panic!("{:?}", e),
        };

        (domains, ck)
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
pub struct SniCertResolver {
    certs: Arc<CertifiedKeyList>,
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        if let Some(server_name) = client_hello.server_name() {
            println!("SNI requested: {}", server_name);

            if let Some(cert) = self.certs.get(&server_name.to_string()) {
                println!("SNI resolved to: {}", server_name);
                return Some(cert.load_full());
            }

            //  Try wildcards.
            let wildcard_name = convert_to_wildcard(&server_name);
            if let Some(cert) = self.certs.get(&wildcard_name) {
                println!("SNI resolved to: {}", wildcard_name);
                return Some(cert.load_full());
            }
        }
        println!("No SNI provided by client.");
        None
    }
}

impl SniCertResolver {
    pub fn new(ck_list: Arc<CertifiedKeyList>) -> SniCertResolver {
        SniCertResolver { certs: ck_list }
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
