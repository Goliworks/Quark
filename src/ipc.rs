use std::{path::PathBuf, sync::Arc};

use bincode::{Decode, Encode};
use nix::unistd::getuid;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Mutex,
    time::{sleep, timeout, Duration},
};

const QUARK_SOCKET_NAME: &str = "quark.sock";
const QUARK_SOCKET_PATH: &str = "/run/quark/";
const QUARK_TMP_SOCKET_PATH: &str = "/tmp/";

pub fn get_socket_path() -> String {
    if getuid().is_root() {
        return PathBuf::from(QUARK_SOCKET_PATH)
            .join(QUARK_SOCKET_NAME)
            .to_string_lossy()
            .to_string();
    }

    PathBuf::from(QUARK_TMP_SOCKET_PATH)
        .join(QUARK_SOCKET_NAME)
        .to_string_lossy()
        .to_string()
}

pub async fn connect_to_socket(socket_path: &str) -> Result<UnixStream, std::io::Error> {
    // Try to connect to the socket for 5 seconds.
    timeout(Duration::from_secs(5), async {
        loop {
            match UnixStream::connect(socket_path).await {
                Ok(stream) => break Ok(stream),
                // Retry after 100ms.
                Err(_) => sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Timed out connecting to socket",
        ))
    })
}

#[derive(Encode, Decode, Debug)]
pub struct IpcMessage<T> {
    pub kind: String,
    pub key: Option<String>,
    pub payload: T,
}

pub async fn send_ipc_message<T>(
    stream: Arc<Mutex<UnixStream>>, //UnixStream,
    message: IpcMessage<T>,
) -> Result<(), Box<dyn std::error::Error>>
where
    T: Encode + Decode<bincode::config::Configuration>,
{
    // Encode the message into vec of bytes.
    let encoded_message = bincode::encode_to_vec(&message, bincode::config::standard())?;
    // Get the size of the message in bytes.
    let message_size: [u8; 4] = (encoded_message.len() as u32).to_be_bytes();
    // First call. Send the size of the message to the child process. (4 bytes)
    stream.lock().await.write_all(&message_size).await?;
    // Second call. Send the message to the child process.
    stream.lock().await.write_all(&encoded_message).await?;
    Ok(())
}

pub async fn receive_ipc_message<T>(
    stream: &mut UnixStream,
) -> Result<IpcMessage<T>, Box<dyn std::error::Error>>
where
    T: Encode + Decode<()>,
{
    // Read the size of the message.
    let mut message_size = [0u8; 4];
    stream.read_exact(&mut message_size).await?;
    // Read the message.
    let buf_size = u32::from_be_bytes(message_size) as usize;
    let mut buf = vec![0u8; buf_size];
    stream.read_exact(&mut buf).await?;
    let (message, _): (IpcMessage<T>, _) =
        bincode::decode_from_slice(&buf, bincode::config::standard())?;
    Ok(message)
}
