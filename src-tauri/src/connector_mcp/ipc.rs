//! Bounded, one-request-per-connection framing for the private local channel.
use super::BridgeResult;
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const MAX_REQUEST: usize = 32 * 1024;
pub(super) const MAX_RESPONSE: usize = 1024 * 1024;

pub(super) async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
    maximum: usize,
) -> BridgeResult<T> {
    let length = reader.read_u32().await.map_err(|_| "ipc_unavailable")? as usize;
    if length == 0 || length > maximum {
        return Err("payload_too_large".into());
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "ipc_unavailable")?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid_request".into())
}

pub(super) async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
    maximum: usize,
) -> BridgeResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| "invalid_response")?;
    if bytes.len() > maximum {
        return Err("payload_too_large".into());
    }
    writer
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "ipc_unavailable")?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| "ipc_unavailable")?;
    writer.flush().await.map_err(|_| "ipc_unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_frame_rejected_before_body_is_read() {
        let (mut sender, mut receiver) = tokio::io::duplex(16);
        sender.write_u32(MAX_REQUEST as u32 + 1).await.unwrap();
        let result = read_frame::<_, serde_json::Value>(&mut receiver, MAX_REQUEST).await;
        assert_eq!(result.unwrap_err(), "payload_too_large");
    }
    #[tokio::test]
    async fn roundtrip_and_unknown_operation_rejected() {
        let (mut sender, mut receiver) = tokio::io::duplex(1024);
        write_frame(
            &mut sender,
            &serde_json::json!({"operation":"approve"}),
            MAX_REQUEST,
        )
        .await
        .unwrap();
        assert!(
            read_frame::<_, super::super::BridgeCall>(&mut receiver, MAX_REQUEST)
                .await
                .is_err()
        );
    }
}
