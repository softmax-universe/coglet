//! Framing and core deterministic CBOR profile (from the shared generated types).
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
#[error("invalid CBOR profile")]
pub struct Invalid;

pub fn encode(value: &Value) -> Result<Vec<u8>, Invalid> {
    let value = ciborium::Value::serialized(value).map_err(|_| Invalid)?;
    coglet_types::encode_profile(&value).map_err(|_| Invalid)
}

pub fn decode(bytes: &[u8]) -> Result<Value, Invalid> {
    coglet_types::decode_profile(bytes)
        .map_err(|_| Invalid)?
        .deserialized()
        .map_err(|_| Invalid)
}

pub const MAX_FRAME: usize = 1048576;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("invalid frame length")]
    Length,
    #[error(transparent)]
    Profile(#[from] Invalid),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads one frame, checking the declared length before allocating its body.
/// Operation owners enforce frame order, request schema and FIN separately.
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Value, FrameError> {
    use tokio::io::AsyncReadExt;
    let len = reader.read_u32().await? as usize;
    if !(1..=MAX_FRAME).contains(&len) {
        return Err(FrameError::Length);
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).await?;
    Ok(decode(&body)?)
}

pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), FrameError> {
    use tokio::io::AsyncWriteExt;
    let body = encode(value)?;
    if !(1..=MAX_FRAME).contains(&body.len()) {
        return Err(FrameError::Length);
    }
    writer.write_u32(body.len() as u32).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}
