use std::sync::Arc;

use bincode::{Decode, Encode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Mutex,
};

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
