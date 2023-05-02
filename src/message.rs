use std::io;

use bytes::BufMut;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::Str;

#[derive(Debug)]
pub enum PizzaMessage {
    Text { body: Str },
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("io {0}")]
    Io(#[from] io::Error),
    #[error("invalid message type {ty}")]
    InvalidType { ty: u8 },
    #[error("utf8 {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

impl PizzaMessage {
    pub async fn read<R: AsyncRead + Unpin>(source: &mut R) -> Result<Self, ParseError> {
        let ty = source.read_u8().await?;
        match ty {
            0 => Self::read_text(source).await,
            _ => Err(ParseError::InvalidType { ty }),
        }
    }

    async fn read_text<R: AsyncRead + Unpin>(source: &mut R) -> Result<Self, ParseError> {
        let vec = {
            let len = source.read_u32().await?;
            let mut buf = Vec::<u8>::with_capacity(len as usize).limit(len as usize);
            while buf.remaining_mut() > 0 {
                source.read_buf(&mut buf).await?;
            }
            buf.into_inner()
        };

        Ok(PizzaMessage::Text {
            body: String::from_utf8(vec)?.into(),
        })
    }
}

impl PizzaMessage {
    pub async fn write<W: AsyncWrite + Unpin>(&self, mut sink: W) -> io::Result<()> {
        match self {
            PizzaMessage::Text { body } => {
                sink.write_u8(0).await?;
                sink.write_u32(body.len() as _).await?;
                sink.write_all(body.as_bytes()).await?;
            }
        }

        Ok(())
    }
}
