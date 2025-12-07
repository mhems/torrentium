use core::fmt;
use std::result::Result;
use std::net::{SocketAddrV4};

use reqwest::get;
use tokio::task::JoinError;

use crate::peer::PEER_ID;
use crate::metadata::file::{TorrentError, TorrentFile};
use crate::metadata::bencode::{BencodeValue, BencodeError};
use crate::peer::message::MessageError;
use crate::peer::downloader::DownloadError;

#[derive(Debug, Clone)]
pub struct TrackerResponse {
    pub interval: u64,
    pub peers: Vec<SocketAddrV4>,
}

#[derive(Debug)]
pub enum ProtocolError {
    InvalidAnnounceUrl,
    NoTrackerResponse(reqwest::Error),
    NoTrackerResponseBody(reqwest::Error),
    InvalidTrackerResponse(TrackerError),
    ReadError(MessageError),
    WriteError(MessageError),
    ConnectionError(std::io::Error),
    HandshakeError(DownloadError),
    Timeout,
    ConversionError,
    JoinError(JoinError),
    DiskError(tokio::io::Error),
    Exhausted,
}

#[derive(Debug)]
pub enum TrackerError {
    NonBencodedTrackerResponse(BencodeError),
    TrackerResponseNotADictionary,
    IllegalPeersLength(usize),
    ByteConversionError,
    MissingInterval,
    MalformedInterval(TorrentError),
    MissingPeers,
    MalformedPeersList,
}

const INTERVAL: &[u8] = b"interval";
const PEERS: &[u8] = b"peers";

fn extract_peers(value: Option<&BencodeValue>) -> Result<Vec<SocketAddrV4>, TrackerError> {
    match value {
        Some(bencoded_value) => {
            match bencoded_value {
                BencodeValue::ByteString(bytes) => {
                    if bytes.len() % 6 != 0 {
                        return Err(TrackerError::IllegalPeersLength(bytes.len()));
                    }
                    let count: usize = bytes.len() / 6;
                    let mut v: Vec<SocketAddrV4> = Vec::with_capacity(count);
                    for i in 0..count {
                        let start = i*6;
                        let end_ip = i*6 + 4;
                        let ip: [u8; 4] = bytes[start..end_ip]
                            .try_into()
                            .map_err(|_| TrackerError::ByteConversionError)?;
                        let port_bytes: [u8; 2] = bytes[end_ip..end_ip+2]
                            .try_into()
                            .map_err(|_| TrackerError::ByteConversionError)?;
                        let port: u16 = u16::from_be_bytes(port_bytes);
                        v.push(SocketAddrV4::new(ip.into(), port));
                    }
                    Ok(v)
                },
                _ => Err(TrackerError::MalformedPeersList),
            }
        },
        None => Err(TrackerError::MissingPeers),
    }
}

impl TryFrom<&BencodeValue> for TrackerResponse {
    type Error = TrackerError;

    fn try_from(value: &BencodeValue) -> Result<Self, TrackerError> {
        match value {
            BencodeValue::Dictionary(items) => {
                let interval: u64 = TorrentFile::extract_uint(items.get(INTERVAL), "interval", true)
                    .map_err(|e| {
                        match e {
                            TorrentError::MissingRequiredKey(_) => TrackerError::MissingInterval,
                            _ => TrackerError::MalformedInterval(e),
                        }
                    })?.unwrap();
                let peers = extract_peers(items.get(PEERS))?;
                Ok(TrackerResponse { interval: interval, peers: peers })
            },
            _ => Err(TrackerError::TrackerResponseNotADictionary),
        }
    }
}

impl fmt::Display for TrackerResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Interval (s): {}\n", self.interval)?;
        for (i, socket) in self.peers.iter().enumerate() {
            write!(f, "{:03}: {}\n", i, socket)?;
        }
        Ok(())
    }
}

pub async fn retrieve_peers(file: &TorrentFile, port: u16) -> Result<TrackerResponse, ProtocolError> {
    let url = file.get_announce_url(PEER_ID, port)
        .map_err(|_| ProtocolError::InvalidAnnounceUrl)?;
        
    let response = get(url).await.map_err(|e| ProtocolError::NoTrackerResponse(e))?;
    let response_bytes: &[u8] = &response.bytes().await.map_err(|e| ProtocolError::NoTrackerResponseBody(e))?;

    let bencoded_response = BencodeValue::try_from(response_bytes)
        .map_err(|e| TrackerError::NonBencodedTrackerResponse(e))
        .map_err(|e| ProtocolError::InvalidTrackerResponse(e))?;

    let tracker_response = TrackerResponse::try_from(&bencoded_response)
        .map_err(|e| ProtocolError::InvalidTrackerResponse(e))?;

    Ok(tracker_response)
}
