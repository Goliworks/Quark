use std::collections::HashMap;
use std::io::{self, BufReader, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bincode::{Decode, Encode};
use futures::{SinkExt, StreamExt};
use notify::event::{AccessKind, AccessMode, ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, Watcher};
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, Notify};
use x509_parser::parse_x509_certificate;
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::{GeneralName, ParsedExtension, X509Certificate};

use futures::channel::mpsc::channel;

use crate::ipc;

use super::TlsCertificate;

pub type CertifiedKeyList = HashMap<String, ArcSwap<CertifiedKey>>;

pub struct TlsConfig<'a> {
    certs: &'a Vec<IpcCerts>,
}

impl<'a> TlsConfig<'a> {
    pub fn new(certs: &'a Vec<IpcCerts>) -> TlsConfig<'a> {
        TlsConfig { certs }
    }

    pub fn get_certified_key_list(&mut self) -> CertifiedKeyList {
        let mut ck_list: CertifiedKeyList = HashMap::new();

        for cert in self.certs.iter() {
            add_certificate_to_certified_key_list(cert, &mut ck_list);
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
}

// Custom SNI resolver.
#[derive(Debug)]
pub struct SniCertResolver {
    certs: Arc<CertifiedKeyList>,
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        if let Some(server_name) = client_hello.server_name() {
            tracing::trace!("SNI requested: {}", server_name);

            if let Some(cert) = self.certs.get(&server_name.to_string()) {
                tracing::trace!("SNI resolved to: {}", server_name);
                return Some(cert.load_full());
            }

            //  Try wildcards.
            let wildcard_name = convert_to_wildcard(&server_name);
            if let Some(cert) = self.certs.get(&wildcard_name) {
                tracing::trace!("SNI resolved to: {}", wildcard_name);
                return Some(cert.load_full());
            }
        }
        tracing::warn!("No SNI provided by client.");
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

fn add_certificate_to_certified_key_list(cert: &IpcCerts, ck_list: &mut CertifiedKeyList) {
    let (domains, ck) = get_domains_and_ck(cert);

    domains.iter().for_each(|domain| {
        ck_list.insert(domain.to_string(), ArcSwap::new(ck.clone()));
    })
}

pub fn reload_certificates(cert: &IpcCerts, ck_list: Arc<CertifiedKeyList>) {
    let (domains, ck) = get_domains_and_ck(cert);

    domains.iter().for_each(|domain| {
        if let Some(ack) = ck_list.get(domain) {
            ack.store(ck.clone());
        }
    });
}

fn get_domains_and_ck(cert: &IpcCerts) -> (Vec<String>, Arc<CertifiedKey>) {
    let cert_buffer = cert.cert.clone();
    let cert_der = load_certs(&cert.cert).unwrap();
    let key = load_private_key(&cert.key).unwrap();

    let key_sign = any_supported_type(&key).unwrap();

    let ck = Arc::new(CertifiedKey::new(cert_der, key_sign));

    let (_, pem) = parse_x509_pem(&cert_buffer).unwrap();

    let domains: Vec<String> = match parse_x509_certificate(&pem.contents) {
        Ok((_, x509_cert)) => extract_domains_from_x509(&x509_cert),
        Err(e) => panic!("{:?}", e),
    };

    (domains, ck)
}

fn extract_domains_from_x509(x509: &X509Certificate) -> Vec<String> {
    let mut domain_names: Vec<String> = Vec::new();
    for ext in x509.extensions() {
        match ext.parsed_extension() {
            ParsedExtension::SubjectAlternativeName(san) => {
                for name in &san.general_names {
                    match name {
                        GeneralName::DNSName(dnsn) => {
                            tracing::trace!("DNSName: {}", dnsn);
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

// Load public certificate from buffer.
fn load_certs(buf: &Vec<u8>) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(Cursor::new(buf));
    // Load and return certificate.
    rustls_pemfile::certs(&mut reader).collect()
}

// Load private key from buffer.
fn load_private_key(buf: &Vec<u8>) -> io::Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(Cursor::new(buf));
    // Load and return a single private key.
    rustls_pemfile::private_key(&mut reader).map(|key| key.unwrap())
}

// Start to watch for certificates changes.
// Run it in a tokio task.
pub async fn watch_certs(
    paths_to_watch: &Vec<PathBuf>,
    port: u16,
    stream: Arc<Mutex<UnixStream>>,
    certs: Vec<TlsCertificate>,
) {
    println!("Watch certificates paths : {:?}", paths_to_watch);

    let (mut tx, mut rx) = channel(1);

    let mut watcher = RecommendedWatcher::new(
        move |res| futures::executor::block_on(async { tx.send(res).await.unwrap() }),
        notify::Config::default(),
    )
    .unwrap();

    for path in paths_to_watch {
        watcher
            .watch(path, notify::RecursiveMode::Recursive)
            .unwrap();
    }

    // Prepare debounce
    let notify = Arc::new(Notify::new());
    let notify_clone = Arc::clone(&notify);
    let debouncing = Arc::new(AtomicBool::new(false));
    let debouncing_clone = debouncing.clone();

    // Watch if file changed
    tokio::spawn(async move {
        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write))
                        || event.kind == EventKind::Modify(ModifyKind::Name(RenameMode::Both))
                    {
                        println!("[Main Process] File changed: {}", event.paths[0].display());
                        if !debouncing.load(Ordering::Relaxed) {
                            // Launch debouncing to avoid to send the files multiple times
                            notify.notify_one();
                            debouncing.store(true, Ordering::Relaxed);
                        }
                    }
                }

                Err(e) => eprintln!("watch error: {:?}", e),
            }
        }
    });

    // Debounce
    loop {
        notify_clone.notified().await;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        // Reload certificates
        let mut cert_list: Vec<IpcCerts> = Vec::new();
        for cert in certs.iter() {
            match IpcCerts::build(&cert.cert, &cert.key).await {
                Ok(certs) => cert_list.push(certs),
                Err(e) => eprintln!("Error. {}", e),
            }
        }

        if cert_list.is_empty() {
            println!("[Parent] No certificates found");
            return;
        }
        let message = ipc::IpcMessage {
            kind: "reload".to_string(),
            key: Some(port.to_string()),
            payload: cert_list,
        };

        ipc::send_ipc_message(stream.clone(), message)
            .await
            .unwrap();
        debouncing_clone.store(false, Ordering::Relaxed);
    }
}

// Struct to send certs via IPC.
#[derive(Encode, Decode, Debug)]
pub struct IpcCerts {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

impl IpcCerts {
    pub async fn build(cert: &str, key: &str) -> Result<IpcCerts, String> {
        let certfile = tokio::fs::read(cert)
            .await
            .map_err(|e| format!("Can't read the certificate {} : {}", cert, e))?;
        let keyfile = tokio::fs::read(key)
            .await
            .map_err(|e| format!("Can't read the key {} : {}", key, e))?;

        Ok(IpcCerts {
            cert: certfile,
            key: keyfile,
        })
    }
}
