//! `u16` little-endian length-prefixed frames on the TCP control channel.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::crypto::TAG_LEN;
use super::message::MAX_MESSAGE;

pub const MAX_FRAME_BODY: usize = MAX_MESSAGE + TAG_LEN;
pub const LEN_PREFIX: usize = 2;

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; LEN_PREFIX];
    r.read_exact(&mut len).await?;
    let len = u16::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad frame length {len}")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(body)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, body: &[u8]) -> io::Result<()> {
    w.write_all(&encode_frame(body)?).await?;
    w.flush().await
}

/// Builds prefix and body in one buffer so each frame is a single write (one TCP segment when small).
pub fn encode_frame(body: &[u8]) -> io::Result<Vec<u8>> {
    if body.is_empty() || body.len() > MAX_FRAME_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad frame length {}", body.len())));
    }
    let mut out = Vec::with_capacity(LEN_PREFIX + body.len());
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_through_a_pipe() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        write_frame(&mut a, b"abc").await.unwrap();
        write_frame(&mut a, &vec![9u8; MAX_FRAME_BODY]).await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"abc");
        assert_eq!(read_frame(&mut b).await.unwrap().len(), MAX_FRAME_BODY);
    }

    #[test]
    fn prefix_is_little_endian() {
        let f = encode_frame(&[0u8; 0x0102]).unwrap();
        assert_eq!(&f[..2], &[0x02, 0x01]);
    }

    #[tokio::test]
    async fn rejects_bad_lengths() {
        assert!(encode_frame(&[]).is_err());
        assert!(encode_frame(&vec![0u8; MAX_FRAME_BODY + 1]).is_err());
        let mut zero: &[u8] = &[0, 0];
        assert_eq!(read_frame(&mut zero).await.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let too_big = ((MAX_FRAME_BODY + 1) as u16).to_le_bytes();
        let mut r: &[u8] = &too_big;
        assert_eq!(read_frame(&mut r).await.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let mut short: &[u8] = &[5, 0, 1, 2];
        assert_eq!(read_frame(&mut short).await.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}
