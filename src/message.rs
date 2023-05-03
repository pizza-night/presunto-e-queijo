use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr},
};

use bytes::BufMut;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::Str;

#[derive(Debug)]
pub enum PizzaMessage {
    Text {
        body: Str,
    },
    SetName {
        name: Str,
    },
    NewPeers {
        ipv4: Vec<Ipv4Addr>,
        ipv6: Vec<Ipv6Addr>,
    },
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
        let msg = match ty {
            0 => Self::read_text(source).await,
            1 => Self::read_set_name(source).await,
            2 => Self::read_new_peers(source).await,
            _ => Err(ParseError::InvalidType { ty }),
        };
        tracing::debug!(ty, ?msg, "received message");
        msg
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

    async fn read_set_name<R: AsyncRead + Unpin>(source: &mut R) -> Result<Self, ParseError> {
        let vec = {
            let len = source.read_u8().await?;
            let mut buf = Vec::<u8>::with_capacity(len as usize).limit(len as usize);
            while buf.remaining_mut() > 0 {
                source.read_buf(&mut buf).await?;
            }
            buf.into_inner()
        };

        Ok(PizzaMessage::SetName {
            name: String::from_utf8(vec)?.into(),
        })
    }

    async fn read_new_peers<R: AsyncRead + Unpin>(source: &mut R) -> Result<Self, ParseError> {
        let ipv4_count = source.read_u8().await?;
        let ipv6_count = source.read_u8().await?;
        let ipv4 = Self::read_ips::<4, Ipv4Addr, _>(source, ipv4_count as _).await?;
        let ipv6 = Self::read_ips::<16, Ipv6Addr, _>(source, ipv6_count as _).await?;
        Ok(Self::NewPeers { ipv4, ipv6 })
    }

    #[tracing::instrument(skip(source))]
    async fn read_ips<const N: usize, Ip, R>(
        source: &mut R,
        ip_count: usize,
    ) -> Result<Vec<Ip>, ParseError>
    where
        R: AsyncRead + Unpin,
        Ip: From<[u8; N]>,
    {
        tracing::trace!(
            "reading {ip_count} ips of type {}",
            std::any::type_name::<Ip>()
        );
        let ip_buf_len = ip_count * N;
        let mut buf = Vec::<u8>::with_capacity(ip_buf_len).limit(ip_buf_len);
        while buf.remaining_mut() > 0 {
            source.read_buf(&mut buf).await?;
        }
        tracing::trace!(buf = ?buf);
        Ok(buf
            .into_inner()
            .chunks_exact(N)
            .map(|bytes| Ip::from(bytes.try_into().unwrap()))
            .collect())
    }
}

impl PizzaMessage {
    pub async fn write<W: AsyncWrite + Unpin>(&self, mut sink: W) -> io::Result<()> {
        tracing::debug!(msg = ?self, "sending message");
        match self {
            PizzaMessage::Text { body } => {
                sink.write_u8(0).await?;
                sink.write_u32(body.len() as _).await?;
                sink.write_all(body.as_bytes()).await?;
            }
            PizzaMessage::SetName { name: body } => {
                sink.write_u8(1).await?;
                sink.write_u8(body.len() as _).await?;
                sink.write_all(body.as_bytes()).await?;
            }
            PizzaMessage::NewPeers { ipv4, ipv6 } => {
                sink.write_u8(2).await?;
                sink.write_u8(ipv4.len() as _).await?;
                sink.write_u8(ipv6.len() as _).await?;
                for ip in ipv4 {
                    sink.write_all(&ip.octets()).await?;
                }
                for ip in ipv6 {
                    sink.write_all(&ip.octets()).await?;
                }
            }
        }

        Ok(())
    }
}
