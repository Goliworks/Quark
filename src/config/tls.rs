use std::fs::File;
use std::io::{BufRead, BufReader};

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

    pub fn get_tls_config(&self) {
        for cert in self.certs.iter() {
            println!("{}", cert.cert);
            self.manage_certificate(cert);
        }
    }

    fn manage_certificate(&self, cert: &TlsCertificate) {
        let cert_file = &mut BufReader::new(File::open(&cert.cert).unwrap());
        let key_file = &mut BufReader::new(File::open(&cert.key).unwrap());

        let cert_buffer = cert_file.fill_buf().unwrap();

        // let certs = rustls_pemfile::certs(cert_file);

        let (_, pem) = parse_x509_pem(&cert_buffer).unwrap();

        match parse_x509_certificate(&pem.contents) {
            Ok((_, x509_cert)) => {
                self.extract_domains_from_x509(&x509_cert);
            }
            Err(e) => println!("{:?}", e),
        }
    }

    fn extract_domains_from_x509(&self, x509: &X509Certificate) {
        for ext in x509.extensions() {
            match ext.parsed_extension() {
                ParsedExtension::SubjectAlternativeName(san) => {
                    for name in &san.general_names {
                        match name {
                            GeneralName::DNSName(dns) => {
                                println!("{}", dns);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
