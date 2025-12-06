use std::result::Result;
use std::net::{SocketAddrV4};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::peer::PEER_ID;

const P_STR: &[u8] = b"BitTorrent protocol";

pub struct Downloader {
    address: SocketAddrV4,
    connection: Option<TcpStream>,
}

pub enum DownloadError {
    NotConnected,
    InvalidHandshakeLength(usize),
    InvalidProtocolIdLength(u8),
    InvalidProtocolId([u8; 19]),
    ByteConversionError,
    TransmissionError(std::io::Error),
    ReceiveError(std::io::Error),
    InsufficientDataReceived(usize),
    MismatchedFlags([u8; 8], [u8; 8]),
    MismatchedHash([u8; 20], [u8; 20]),
    MismatchedPeerId([u8; 20], [u8; 20]),
}

#[derive(Debug, PartialEq, Eq)]
struct TorrentHandshake {
    flags: [u8; 8],
    info_hash: [u8; 20],
    peer_id: [u8; 20],
}

impl TorrentHandshake {
    fn new(info_hash: &[u8; 20]) -> Self {
        TorrentHandshake { 
            flags: [0; 8],
            info_hash: info_hash.to_owned(),
            peer_id: *PEER_ID
        }
    }
}

impl TryFrom<&[u8]> for TorrentHandshake {
    type Error = DownloadError;
    fn try_from(bytes: &[u8]) -> Result<Self, DownloadError> {
        if bytes.len() != 68 {
            return Err(DownloadError::InvalidHandshakeLength(bytes.len()));
        };
        if bytes[0] != 19 {
            return Err(DownloadError::InvalidProtocolIdLength(bytes[0]));
        };
        if bytes[1..20] != *P_STR {
            let p_str = bytes[1..20].try_into().map_err(|_| DownloadError::ByteConversionError)?;
            return Err(DownloadError::InvalidProtocolId(p_str));
        }
        let flags: [u8; 8] = bytes[20..28].try_into().map_err(|_| DownloadError::ByteConversionError)?;
        let info_hash: [u8; 20] = bytes[28..48].try_into().map_err(|_| DownloadError::ByteConversionError)?;
        let peer_id: [u8; 20] = bytes[48..60].try_into().map_err(|_| DownloadError::ByteConversionError)?;
        Ok(TorrentHandshake {flags, info_hash, peer_id})
    }
}

impl From<&TorrentHandshake> for [u8; 68] {
    fn from(handshake: &TorrentHandshake) -> [u8; 68] {
        let mut bytes: [u8; 68] = [0; 68];
        bytes[0] = 0x13;
        bytes[1..20].copy_from_slice(P_STR);
        bytes[20..28].copy_from_slice(&handshake.flags);
        bytes[28..48].copy_from_slice(&handshake.info_hash);
        bytes[48..68].copy_from_slice(PEER_ID);
        bytes
    }
}

impl Downloader {
    pub fn new(address: SocketAddrV4) -> Self {
        Downloader { address, connection: None }
    }

    pub async fn connect(&mut self) -> std::io::Result<()> {
        self.connection = Some(TcpStream::connect(self.address).await?);
        Ok(())
    }

    pub async fn handshake(&mut self, info_hash: &[u8; 20]) -> Result<(), DownloadError> {
        let mine = TorrentHandshake::new(info_hash);
        let my_bytes = <[u8;68]>::from(&mine);

        if let Some(stream) = &mut self.connection {
            stream.write_all(my_bytes.as_slice()).await.map_err(|e| DownloadError::TransmissionError(e))?;

            let mut buf: [u8; 68] = [0; 68];
            let num_read = stream.read_exact(&mut buf).await.map_err(|e| DownloadError::ReceiveError(e))?;

            if num_read < buf.len() {
                let slice: &[u8] = &buf;
                let theirs = TorrentHandshake::try_from(slice)?;

                if mine.flags != theirs.flags {
                    Err(DownloadError::MismatchedFlags(mine.flags, theirs.flags))
                } else if mine.info_hash != theirs.info_hash {
                    Err(DownloadError::MismatchedHash(mine.info_hash, theirs.info_hash))
                } else if mine.peer_id != theirs.peer_id {
                    Err(DownloadError::MismatchedPeerId(mine.peer_id, theirs.peer_id))
                } else {
                    Ok(())
                }
            }
            else {
                Err(DownloadError::InsufficientDataReceived(num_read))
            }
        }
        else {
            Err(DownloadError::NotConnected)
        }
    }
}