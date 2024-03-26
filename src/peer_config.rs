use std::{
    fmt::Display,
    io,
    net::{SocketAddr, ToSocketAddrs},
    path::Path,
};

use thiserror::Error;
use tokio::fs;

use crate::Str;

pub type Peers = Vec<(Option<Str>, SocketAddr)>;

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
            line = self.line_no + 1,
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
    #[error("failed to resolve host {host} because {error}")]
    FailedToResolve { host: String, error: io::Error },
}

pub async fn load<P: AsRef<Path>>(path: P) -> Result<Peers, LoadConfigError> {
    let contents = fs::read_to_string(path).await?;
    let peers = contents
        .lines()
        .map(|l| l.trim())
        .enumerate()
        .map(|(line_no, line)| {
            let Some((name, ip)) = line.split_once('|') else {
                return Err(ConfigParseError {
                    line_no,
                    column: line.chars().count().wrapping_sub(1),
                    line: line.to_owned(),
                    kind: ConfigParseErrorKind::MissingColon,
                });
            };

            let column = || name.chars().count() + 1;
            let ip = match ip
                .to_socket_addrs()
                .or_else(|_| (ip, 2504u16).to_socket_addrs())
            {
                Ok(mut addrs) => Ok(addrs.next().unwrap()),
                Err(e) if e.kind() == io::ErrorKind::InvalidInput => Err(ConfigParseError {
                    line_no,
                    column: column(),
                    line: line.to_owned(),
                    kind: ConfigParseErrorKind::InvalidIp {
                        found: ip.to_owned(),
                    },
                }),
                Err(e) => Err(ConfigParseError {
                    line_no,
                    column: column(),
                    line: line.to_owned(),
                    kind: ConfigParseErrorKind::FailedToResolve {
                        host: ip.to_owned(),
                        error: e,
                    },
                }),
            }?;
            Ok((Some(name.into()), ip))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(peers)
}
