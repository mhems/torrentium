mod client;
mod handshake;
mod message;
mod downloader;

use std::sync::Arc;

use crate::{metadata::file::{FileModeInfo::{Multiple, Single}, TorrentFile}, peer::downloader::{FileDownloadInfo, FileDownloadState}};
use crate::peer::{client::retrieve_peers, downloader::Downloader};

pub use downloader::DownloadError;
use tokio::sync::Mutex;

const PEER_ID: &[u8; 20] = b"!MySuperCoolTorrent!";

#[derive(Debug)]
pub struct Bitfield {
    masks: Vec<u8>,
    pub num: usize,
    last_mask: u8,
}

impl From<Vec<u8>> for Bitfield {
    fn from(v: Vec<u8>) -> Self {
        let num = v.len() * 8;
        Bitfield { masks: v, num, last_mask: 0xFF }
    }
}

impl Bitfield {
    pub fn new(num: usize, set: bool) -> Self {
        let num_elements = num.div_ceil(8);
        let masks = if set { vec![1; num_elements] } else { vec![0; num_elements] };
        Bitfield { masks, num, last_mask: 0xFF}
    }

    pub fn from_vec(v: Vec<u8>, num: usize) -> Self {
        let num_elements = num.div_ceil(8);
        if v.len() < num_elements {
            panic!("cannot represent a Bitfield with {} fields in a vector with {} elements", num, v.len());
        }
        let extra = num_elements % 8;
        let last_mask: u8 = if extra != 0 { ((1 << extra) - 1) << (8 - extra) } else { 0xFF };
        let mut bf = Bitfield { masks: v, num, last_mask };
        if extra != 0 {
            if let Some(last) = bf.masks.last_mut() {
                *last &= last_mask;
            }
        }
        bf
    }

    fn index_check(&self, index: usize) {
        if index >= self.num {
            panic!("Bitfield only represents {} fields", self.num);
        }
    }

    pub fn has_piece(&self, index: usize) -> bool {
        self.index_check(index);
        let element_index = index / 8;
        let element_offset = index % 8;
        let mask = 1 << (7 - element_offset);
        self.masks[element_index] & mask == mask
    }
    
    pub fn mark_piece(&mut self, index: usize) {
        self.index_check(index);
        let element_index = index / 8;
        let element_offset = index % 8;
        let mask = 1 << (7 - element_offset);
        self.masks[element_index] |= mask;
    }

    pub fn ummark_piece(&mut self, index: usize) {
        self.index_check(index);
        let element_index = index / 8;
        let element_offset = index % 8;
        let mask = 1 << (7 - element_offset);
        self.masks[element_index] &= !mask;
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


pub async fn download(file: &TorrentFile, port: u16, num_seeds: usize) -> Result<(), DownloadError> {
    match &file.info.as_ref() {
        Single { .. } => download_file(file, port, num_seeds).await?,
        Multiple { directory, files } => {
            panic!("multiple files currently not supported");
        }
    };
    Ok(())
}

pub async fn download_file(file: &TorrentFile, port: u16, num_seeds: usize) -> Result<(), DownloadError> {
    match file.info.as_ref() {
        Single {filename, .. } => {
            let tracker_response = retrieve_peers(file, port).await
                .map_err(|_| DownloadError::RetrievalError)?;
            let n = num_seeds.min(tracker_response.peers.len());
            println!("using {} seeds (of {})", n, tracker_response.peers.len());

            let mut tasks = Vec::with_capacity(n);
            let filename_arc: Arc<String> = Arc::new(filename.into());

            let info = FileDownloadInfo::try_from(file)?;
            let info_arc = Arc::new(info);

            let state = FileDownloadState::new(file.num_pieces);

            let state_arc = Arc::new(Mutex::new(state));

            for i in 0..n {
                let peer_clone = tracker_response.peers[i].clone();
                let info_clone = info_arc.clone();
                let state_clone = state_arc.clone();
                
                println!("spawning task to download '{}' from {}", filename_arc, peer_clone);

                tasks.push(tokio::spawn(async move {
                    let mut downloader = Downloader::new(
                        peer_clone,
                        info_clone,
                        state_clone
                    );
                    downloader.download_file().await
                }));
            }

            for (i, task) in tasks.into_iter().enumerate() {
                match task.await {
                    Ok(Ok(())) => (),
                    Ok(Err(e)) => println!("[{}]: error with task: {:?}", tracker_response.peers[i], e),
                    Err(e) => println!("[{}]: error with join: {:?}", tracker_response.peers[i], e),
                }
            }
        },
        Multiple { directory, files } => {
            panic!("multiple files currently not supported");
        }
    };
    Ok(())
}
