use core::fmt;
use std::result::Result;
use std::net::{SocketAddrV4};

use reqwest::get;
use thiserror::Error;
use url::Url;

use crate::metadata::file::{TorrentFileError, TorrentFile};
use crate::metadata::bencode::{BencodeValue, BencodeError};

#[derive(Debug, Clone)]
pub struct TrackerResponse {
    pub interval: u64,
    pub peers: Vec<SocketAddrV4>,
}

#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("tracker response is invalid bencode data: {0:?}")]
    NonBencodedTrackerResponse(BencodeError),
    #[error("tracker response is not a bencoded dictionary")]
    TrackerResponseNotADictionary,
    #[error("peers list byte length ({0}) is not a multiple of 6")]
    IllegalPeersLength(usize),
    #[error("tracker response missing interval key")]
    MissingInterval,
    #[error("tracker response interval malformed: {0:?}")]
    MalformedInterval(TorrentFileError),
    #[error("tracker repsonse missing peers key")]
    MissingPeers,
    #[error("tracker response peers list is not a byte string")]
    MalformedPeersList,
    #[error("no response received from tracker: {0:?}")]
    NoTrackerResponse(reqwest::Error),
    #[error("tracker response contains no body: {0:?}")]
    NoTrackerResponseBody(reqwest::Error),
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
                            .expect("slice expected to be length 4");
                        let port_bytes: [u8; 2] = bytes[end_ip..end_ip+2]
                            .try_into()
                            .expect("slice expected to be length 2");
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
                            TorrentFileError::MissingRequiredKey(_) => TrackerError::MissingInterval,
                            _ => TrackerError::MalformedInterval(e),
                        }
                    })?.unwrap();
                let peers = extract_peers(items.get(PEERS))?;
                Ok(TrackerResponse { interval, peers })
            },
            _ => Err(TrackerError::TrackerResponseNotADictionary),
        }
    }
}

impl fmt::Display for TrackerResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Interval (s): {}", self.interval)?;
        for (i, socket) in self.peers.iter().enumerate() {
            writeln!(f, "{i:03}: {socket}\n")?;
        }
        Ok(())
    }
}

pub async fn retrieve_peers(url: Url) -> Result<TrackerResponse, TrackerError> {      
    let response = get(url).await.map_err(TrackerError::NoTrackerResponse)?;
    let response_bytes: &[u8] = &response.bytes().await.map_err(TrackerError::NoTrackerResponseBody)?;

    let bencoded_response = BencodeValue::try_from(response_bytes)
        .map_err(TrackerError::NonBencodedTrackerResponse)?;

    let tracker_response = TrackerResponse::try_from(&bencoded_response)?;

    Ok(tracker_response)
}
