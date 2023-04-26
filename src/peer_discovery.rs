use std::{
    fmt::Display,
    io,
    net::{IpAddr, SocketAddr},
    path::Path,
};

use thiserror::Error;
use tokio::fs;

use crate::message::ArcStr;

#[derive(Debug)]
pub struct Peer {
    pub name: ArcStr,
    pub addr: SocketAddr,
}

#[derive(Debug)]
pub struct Peers {
    pub peers: Vec<Peer>,
}

#[derive(Error, Debug)]
pub enum LoadConfigError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Parsing(#[from] ConfigParseError),
}

#[derive(Error, Debug)]
pub struct ConfigParseError {
    line_no: usize,
    column: usize,
    line: String,
    kind: ConfigParseErrorKind,
}

impl Display for ConfigParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "config parse error at {line}:{col}: {kind}",
            line = self.line_no,
            col = self.column,
            kind = self.kind,
        )?;
        writeln!(f, "{}", self.line)?;
        for _ in 0..self.column {
            write!(f, " ")?;
        }
        write!(f, "^")
    }
}

#[derive(Error, Debug)]
pub enum ConfigParseErrorKind {
    #[error("missing colon")]
    MissingColon,
    #[error("invalid ip, found {found:?}")]
    InvalidIp { found: String },
}

pub async fn from_config<P: AsRef<Path>>(path: P) -> Result<Peers, LoadConfigError> {
    let contents = fs::read_to_string(path).await?;
    let peers = contents
        .lines()
        .map(|l| l.trim())
        .enumerate()
        .map(|(line_no, line)| {
            let Some((name, ip)) =  line.split_once('|') else {
                return Err(ConfigParseError {
                    line_no,
                    column: line.chars().count().wrapping_sub(1),
                    line: line.to_owned(),
                    kind: ConfigParseErrorKind::MissingColon
                });
            };

            let ip = ip
                .parse::<SocketAddr>()
                .or_else(|_| ip.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 2504)))
                .map_err(|_| ConfigParseError {
                    line_no,
                    column: name.chars().count() + 1,
                    line: line.to_owned(),
                    kind: ConfigParseErrorKind::InvalidIp {
                        found: ip.to_owned(),
                    },
                })?;
            Ok(Peer {
                name: name.into(),
                addr: ip,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Peers { peers })
}
