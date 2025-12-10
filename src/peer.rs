mod client;
mod handshake;
mod message;
mod downloader;

use std::{path::{PathBuf}, sync::Arc, fs};

use crate::metadata::file::{FileModeInfo::{Multiple, Single}, TorrentFile};
use crate::peer::{client::{ProtocolError, retrieve_peers}, downloader::{FileDownloadInfo, FileDownloadState, Downloader}};
use crate::util::{self, md5::md5_hash};

pub use downloader::DownloadError;

use tempfile::TempDir;
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

#[derive(Debug)]
pub enum BitfieldError {
    Unrepresentible{num_fields: usize, num_elements: usize},
    PieceOutOfRange{index: usize, len: usize},
}

impl Bitfield {
    pub fn new(num: usize, set: bool) -> Self {
        let num_elements = num.div_ceil(8);
        let masks = if set { vec![0xFF; num_elements] } else { vec![0; num_elements] };
        Bitfield { masks, num, last_mask: 0xFF}
    }

    pub fn try_from_vec(v: Vec<u8>, num: usize) -> Result<Self, BitfieldError> {
        let num_elements = num.div_ceil(8);
        if v.len() < num_elements {
            return Err(BitfieldError::Unrepresentible { num_fields: num, num_elements: v.len() });
        }
        let extra = num_elements % 8;
        let last_mask: u8 = if extra != 0 { ((1 << extra) - 1) << (8 - extra) } else { 0xFF };
        let mut bf = Bitfield { masks: v, num, last_mask };
        if extra != 0 {
            if let Some(last) = bf.masks.last_mut() {
                *last &= last_mask;
            }
        }
        Ok(bf)
    }

    fn index_check(&self, index: usize) -> Result<(), BitfieldError> {
        if index >= self.num {
            Err(BitfieldError::PieceOutOfRange { index, len: self.num })
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

pub async fn download(file: &TorrentFile, port: u16, num_seeds: usize) -> Result<(), DownloadError> {
    match &file.info {
        Single { .. } => download_file(file, port, num_seeds).await?,
        Multiple { directory, files } => {
            panic!("multiple files currently not supported");
        }
    };
    Ok(())
}

pub async fn download_file(file: &TorrentFile, port: u16, num_seeds: usize) -> Result<(), DownloadError> {
    match &file.info {
        Single {filename, length: _, md5sum } => {
            let tracker_response = retrieve_peers(file, port).await
                .map_err(|_| DownloadError::RetrievalError)?;
            let n = num_seeds.min(tracker_response.peers.len());
            println!("using {} seeds (of {})", n, tracker_response.peers.len());

            let mut tasks = Vec::with_capacity(n);
            
            let path = PathBuf::from(filename);
            let stem = path.file_stem().unwrap().to_string_lossy();

            let dir = TempDir::new().map_err(|e| DownloadError::FileSystemError(e))?;
            let dir_path = dir.path().to_path_buf();

            println!("temp dir is {}", dir_path.to_string_lossy());

            let basename = stem.as_ref().to_string();
            let basename_arc = Arc::new(basename);

            let info = FileDownloadInfo::try_from(file)?;
            let info_arc: Arc<FileDownloadInfo> = Arc::new(info);

            let state = FileDownloadState::new(file.num_pieces);
            let state_arc = Arc::new(Mutex::new(state));

            for i in 0..n {
                let peer_clone = tracker_response.peers[i].clone();
                let info_clone = info_arc.clone();
                let state_clone = state_arc.clone();
                let dir_clone = dir_path.clone();
                let basename_clone = basename_arc.clone();
                
                println!("spawning task to download '{}' from {}", filename, peer_clone);

                tasks.push(tokio::spawn(async move {
                    let mut downloader = Downloader::new(
                        peer_clone,
                        info_clone,
                        state_clone,
                        dir_clone,
                        basename_clone
                    ).await.map_err(|e| ProtocolError::ConnectionError(e))?;
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

            let paths: Vec<PathBuf> = (0..file.num_pieces)
                .map(|i| dir_path.join(format!("{}.{}.bin", stem, i)))
                .collect();

            util::io::concatenate_pieces(&paths, &path)
                .map_err(|e| DownloadError::FileSystemError(e))?;

            if let Some(expected_hash) = md5sum {
                let bytes = fs::read(&path).map_err(DownloadError::FileSystemError)?;
                let downloaded_hash = md5_hash(&bytes);
                if downloaded_hash == *expected_hash {
                    Ok(())
                } else {
                    Err(DownloadError::Md5Mismatch)
                }
            } else {
                Ok(())
            }
        },
        Multiple { directory, files } => {
            panic!("multiple files currently not supported");
        }
    }
}
