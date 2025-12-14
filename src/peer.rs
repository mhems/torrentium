pub mod handshake;
pub mod message;
pub mod downloader;

use std::path::Path;
use std::{net::SocketAddrV4, sync::Arc};

use crate::metadata::file::TorrentFile;
use crate::peer::downloader::{FileDownloadInfo, FileDownloadState, Downloader};

use tokio::sync::Mutex;
use thiserror::Error;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::{info, error};

#[derive(Debug, Error)]
pub enum PeerError {
    #[error("unable to connect to peer {0}: {1:?}")]
    ConnectionError(String, std::io::Error),

    #[error("unable to send handshake to peer {0}: {1:?}")]
    HandshakeTransmissionError(String, tokio::io::Error),
    #[error("did not receive handshake response from peer {0}: {1:?}")]
    HandshakeReceiveError(String, tokio::io::Error),
    #[error("expected peer handshake response to be 68 bytes but was {0} bytes")]
    InvalidHandshakeLength(usize),
    #[error("expected peer handshake response to have a protocol ID of length 19 bytes but was {0} bytes")]
    InvalidProtocolIdLength(u8),
    #[error("expected peer handshake response protocol to be `BitTorrent protocol` but was {0:?}")]
    InvalidProtocolId([u8; 19]),
    #[error("peer file hash ({0:?}) does not match requested file hash ({1:?})")]
    MismatchedHash([u8; 20], [u8; 20]),

    #[error("unknown message id {0}")]
    UnknownMessageId(u8),
    #[error("error encountered while reading {1} bytes: {0:?}")]
    MessageReceiveError(tokio::io::Error, usize),
    #[error("error encountered while sending {1} bytes: {0:?}")]
    MessageTransmitError(tokio::io::Error, usize),
    #[error("expected Piece message to have at least 8 bytes but only received {0} bytes")]
    PieceMessageTooSmall(usize),

    #[error("peer {0} has no more pieces available")]
    Exhausted(String),

    #[error("unable to save piece {0} to disk: {1:?}")]
    DiskError(u32, tokio::io::Error),
}

#[derive(Debug)]
pub struct Bitfield {
    masks: Vec<u8>,
    pub num: usize,
    last_mask: u8,
}

impl From<Vec<u8>> for Bitfield {
    fn from(v: Vec<u8>) -> Self {
        let num: usize = 8 * (v.len() - 1) + v.last().unwrap().count_ones() as usize;
        Bitfield::from_vec(v, num)
    }
}

#[derive(Debug, Error)]
pub enum BitfieldError {
    #[error("cannot represent {num_fields} bits with only {num_elements} bytes")]
    Unrepresentible{num_fields: usize, num_elements: usize},
    #[error("piece {0} is out of range of this Bitfield")]
    PieceOutOfRange(usize),
}

impl Bitfield {
    pub fn new(num: usize, set: bool) -> Self {
        let num_elements = num.div_ceil(8);
        let masks: Vec<u8> = if set { vec![0xFF; num_elements] } else { vec![0; num_elements] };
        Self::from_vec(masks, num)
    }

    pub fn try_from_vec(v: Vec<u8>, num: usize) -> Result<Self, BitfieldError> {
        let num_elements = num.div_ceil(8);
        if v.len() < num_elements {
            return Err(BitfieldError::Unrepresentible { num_fields: num, num_elements: v.len() });
        }
        Ok(Self::from_vec(v, num))
    }

    fn from_vec(v: Vec<u8>, num: usize) -> Self {
        let extra = num % 8;
        let last_mask: u8 = if extra != 0 { ((1 << extra) - 1) << (8 - extra) } else { 0xFF };
        let mut bf = Bitfield { masks: v, num, last_mask };
        if extra != 0 {
            if let Some(last) = bf.masks.last_mut() {
                *last &= last_mask;
            }
        }
        bf
    }

    fn index_check(&self, index: usize) -> Result<(), BitfieldError> {
        if index >= self.num {
            Err(BitfieldError::PieceOutOfRange(index))
        } else {
            Ok(())
        }
    }

    pub fn has_piece(&self, index: usize) -> Result<bool, BitfieldError> {
        self.index_check(index)?;
        let element_index = index / 8;
        let element_offset = index % 8;
        let mask = 1 << (7 - element_offset);
        Ok(self.masks[element_index] & mask == mask)
    }
    
    pub fn mark_piece(&mut self, index: usize) -> Result<(), BitfieldError> {
        self.index_check(index)?;
        let element_index = index / 8;
        let element_offset = index % 8;
        let mask = 1 << (7 - element_offset);
        self.masks[element_index] |= mask;
        Ok(())
    }

    pub fn ummark_piece(&mut self, index: usize) -> Result<(), BitfieldError> {
        self.index_check(index)?;
        let element_index = index / 8;
        let element_offset = index % 8;
        let mask = 1 << (7 - element_offset);
        self.masks[element_index] &= !mask;
        Ok(())
    }

    pub fn num_set(&self) -> usize {
        let mut num: usize = 0;
        for mask in &self.masks {
            num += mask.count_ones() as usize;
        }
        num
    }

    pub fn num_unset(&self) -> usize {
        self.num - self.num_set()
    }

    pub fn all(&self) -> bool {
        let n = self.masks.len() - 1;
        self.masks[n] == self.last_mask && self.masks[..n].iter().all(|&e| e == 0xFF)
    }

    pub fn none(&self) -> bool {
        self.masks.iter().all(|&e| e == 0x00)
    }
}

pub async fn download(
    peers: &[SocketAddrV4],
    file: &TorrentFile,
    dir_path: &Path,
    ) -> Result<(), PeerError> {
    let mut tasks = Vec::with_capacity(peers.len());
    let dir_arc = Arc::new(dir_path.to_path_buf());
    let info = FileDownloadInfo::from(file);
    let info_arc: Arc<FileDownloadInfo> = Arc::new(info);
    let state = FileDownloadState::new(file.num_pieces);
    let state_arc = Arc::new(Mutex::new(state));

    let pb = ProgressBar::new(file.total_num_bytes);

    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] (ETA: {eta}) [{bar:40.cyan/blue}] ({percent}%) {bytes}/{total_bytes} ({bytes_per_sec})")
            .unwrap(),
    );

    for peer in peers {
        let peer_copy = *peer;
        let info_clone = info_arc.clone();
        let state_clone = state_arc.clone();        
        let dir_clone = dir_arc.clone();
        let pb_clone = pb.clone();
        
        info!("spawning task to collaboratively download '{}' from {}", &file.filename, peer_copy);

        tasks.push(tokio::spawn(async move {
            let mut downloader = Downloader::new(
                peer_copy,
                info_clone,
                state_clone,
                dir_clone,
                pb_clone
            ).await.map_err(|e| PeerError::ConnectionError(peer_copy.to_string(), e))?;
            downloader.download_pieces().await
        }));
    }

    for (i, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(())) => info!("... exiting"),
            Ok(Err(e)) => error!("peer {} took error {:?}", peers[i], e),
            Err(e) => error!("peer {} took error {:?}", peers[i], e),
        }
    }

    info!("download of {} complete", file.filename);
    pb.finish();

    Ok(())
}
