use std::fmt;
use std::net::SocketAddrV4;
use std::result::Result;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::PEER_ID;
use crate::metadata::bencode::{write_byte_string, write_bytes};
use crate::peer::PeerError;

const P_STR: &[u8] = b"BitTorrent protocol";

#[derive(Debug)]
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
    type Error = PeerError;
    fn try_from(bytes: &[u8]) -> Result<Self, PeerError> {
        if bytes.len() != 68 {
            return Err(PeerError::InvalidHandshakeLength(bytes.len()));
        }
        if bytes[0] != 19 {
            return Err(PeerError::InvalidProtocolIdLength(bytes[0]));
        }
        if bytes[1..20] != *P_STR {
            let p_str = bytes[1..20].try_into().expect("bytes verified to be length 68");
            return Err(PeerError::InvalidProtocolId(p_str));
        }
        let flags: [u8; 8] = bytes[20..28].try_into().expect("bytes verified to be length 68");
        let info_hash: [u8; 20] = bytes[28..48].try_into().expect("bytes verified to be length 68");
        let peer_id: [u8; 20] = bytes[48..68].try_into().expect("bytes verified to be length 68");
        Ok(TorrentHandshake {flags, info_hash, peer_id})
    }
}

impl fmt::Display for TorrentHandshake {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "peer id: ")?;
        write_byte_string(&self.peer_id, f)?;
        write!(f, ", flags: 0x")?;
        write_bytes(&self.flags, f)
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

pub(crate) async fn handshake(address: &SocketAddrV4, stream: &mut TcpStream, info_hash: &[u8; 20]) -> Result<(), PeerError> {
    let mine = TorrentHandshake::new(info_hash);
    let my_bytes = <[u8;68]>::from(&mine);
    stream.write_all(my_bytes.as_slice()).await.map_err(|e| PeerError::HandshakeTransmissionError(address.to_string(), e))?;
    let mut buf: [u8; 68] = [0; 68];
    stream.read_exact(&mut buf).await.map_err(|e| PeerError::HandshakeReceiveError(address.to_string(), e))?;

    let slice: &[u8] = &buf;
    let theirs = TorrentHandshake::try_from(slice)?;
    if mine.info_hash != theirs.info_hash {
        Err(PeerError::MismatchedHash(mine.info_hash, theirs.info_hash))
    } else {
        // log("handshaked with {}", &theirs);
        Ok(())
    }
}