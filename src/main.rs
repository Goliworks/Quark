mod config;
mod http_response;
mod ipc;
mod logs;
mod server;
mod utils;

use std::collections::HashMap;
use std::fs::{set_permissions, Permissions};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use config::tls::{self, IpcCerts};
use config::{Options, ServiceConfig};

use nix::unistd::{getuid, User};
use tokio::sync::Mutex;
use utils::QUARK_USER_AND_GROUP;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // If the child process flag is set, run the server as a child process.
    if std::env::args().any(|arg| arg == "--child-process") {
        return server::server_process().await;
    }

    // If not, run a new process flagged as a child process.

    let socket_path = ipc::get_socket_path();
    // Clean the socket file if it exists.
    if std::path::Path::new(&socket_path).exists() {
        println!("[Main Process] Removing socket file");
        std::fs::remove_file(&socket_path)?;
    }

    // Take the rest of the arguments and pass them to the child process.
    let mut child_args: Vec<String> = std::env::args().skip(1).collect();
    child_args.insert(0, "--child-process".to_string());

    // Create the child process.
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .args(child_args)
        .spawn()?;

    // Run the main process.
    main_process().await?;
    child.wait()?;
    Ok(())
}

async fn main_process() -> Result<(), Box<dyn std::error::Error>> {
    // Create a unix socket listener.
    let socket_path = ipc::get_socket_path();
    let listener = tokio::net::UnixListener::bind(&socket_path)
        .map_err(|e| format!("Can't use the socket at {} : {}", &socket_path, e))?;

    if getuid().is_root() {
        // Change the owner and permissions of the socket file.
        let quark_user = User::from_name(QUARK_USER_AND_GROUP)
            .expect("User lookup failed")
            .expect("User not found");

        let path = Path::new(&socket_path);
        chown(
            path,
            Some(quark_user.uid.as_raw()),
            Some(quark_user.gid.as_raw()),
        )?;
        set_permissions(&socket_path, Permissions::from_mode(0o600))?;
    }

    println!("[Main Process] Waiting for connection");
    let (stream, _) = listener.accept().await?;
    let stream = Arc::new(Mutex::new(stream));
    println!("[Main Process] Connection accepted");

    // Get options from command line.
    let options: Options = argh::from_env();
    // Load the config file.
    let service_config = ServiceConfig::build_from(options.config);

    let mut paths_to_watch_list: HashMap<u16, Vec<PathBuf>> = HashMap::new();
    let mut cert_list: HashMap<u16, Vec<IpcCerts>> = HashMap::new();
    let mut tls_servers: HashMap<u16, Vec<config::TlsCertificate>> = HashMap::new();

    for (port, server) in &service_config.servers {
        if let Some(tls_certs) = &server.tls {
            tls_servers.insert(*port, tls_certs.clone());
            println!("[Main Process] Server {} is configured with TLS", port);
            println!("[Main Process] tls {:#?}", tls_certs);
            for cert in tls_certs {
                // Add the certificates path to the list of paths to watch.
                let path = Path::new(&cert.cert);
                // Check if the file is a symlink.
                if path.is_symlink() {
                    // If it is, add the target of the symlink to the list of paths to watch.
                    let target = std::fs::canonicalize(path).unwrap();
                    let directory = target.parent().unwrap();
                    let pathbuf = directory.to_path_buf();
                    let paths_to_watch = paths_to_watch_list.entry(*port).or_default();
                    if !paths_to_watch.contains(&pathbuf) {
                        paths_to_watch.push(pathbuf);
                    }
                }
                // Add the directory of the file to the list of paths to watch.
                let directory = path.parent().unwrap();
                let pathbuf = directory.to_path_buf();
                let paths_to_watch = paths_to_watch_list.entry(*port).or_default();
                if !paths_to_watch.contains(&pathbuf) {
                    paths_to_watch.push(pathbuf);
                }
                // Read the certificate and the key.
                match IpcCerts::build(&cert.cert, &cert.key).await {
                    Ok(certs) => {
                        cert_list.entry(*port).or_default().push(certs);
                    }
                    Err(e) => panic!("Error. {}", e),
                }
            }
        }
    }

    println!("[Main Process] paths to watch {:#?}", paths_to_watch_list);

    // Send the config to the child process.
    let message = ipc::IpcMessage {
        kind: "config".to_string(),
        key: None,
        payload: service_config,
    };
    ipc::send_ipc_message(stream.clone(), message).await?;

    // Send the certs to the child process.
    let message = ipc::IpcMessage {
        kind: "certs".to_string(),
        key: None,
        payload: cert_list,
    };
    ipc::send_ipc_message(stream.clone(), message).await?;

    // Watch certificates
    for (port, paths_to_watch) in paths_to_watch_list {
        let stream = Arc::clone(&stream);
        let certs = tls_servers.get(&port).unwrap().clone();
        tokio::task::spawn(async move {
            tls::watch_certs(&paths_to_watch, port, stream, certs).await;
        });
    }
    Ok(())
}
