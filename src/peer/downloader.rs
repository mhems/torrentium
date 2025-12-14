use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::sync::Arc;

use indicatif::ProgressBar;
use tokio::net::TcpStream;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use std::path::{Path, PathBuf};
use tracing::{info, error};

use crate::metadata::file::TorrentFile;
use crate::peer::{Bitfield, PeerError};
use crate::peer::handshake::handshake;
use crate::peer::message::Message;
use crate::util::sha1::sha1_hash;
use crate::util::to_string;

const BLOCK_SIZE: u32 = 16 * 1024;

#[derive(Debug)]
pub struct Downloader {
    pub address: SocketAddrV4,
    connection: TcpStream,
    info: Arc<FileDownloadInfo>,
    shared_state: Arc<Mutex<FileDownloadState>>,
    skip_set: HashSet<u32>,
    state: State,
    dir: Arc<PathBuf>,
    pb: ProgressBar,
}

#[derive(Debug, Clone)]
pub struct FileDownloadInfo {
    bytes_per_piece: usize,
    piece_hashes: Vec<[u8; 20]>,
    hash: [u8; 20],
}

#[derive(Debug)]
pub struct FileDownloadState {
    done: Bitfield,
    todo: HashSet<u32>,
}

#[derive(Debug)]
struct PieceDownloadProgress {
    offset: u32,
    data: Vec<u8>,
}

impl From<&TorrentFile> for FileDownloadInfo {
    fn from(file: &TorrentFile) -> Self {
        FileDownloadInfo {
            bytes_per_piece: file.num_bytes_per_piece as usize,
            piece_hashes: file.piece_hashes.clone(),
            hash: file.hash.clone()
        }
    }
}

impl FileDownloadState {
    pub fn new(num_pieces: usize) -> Self {
        FileDownloadState {
            done: Bitfield::new(num_pieces, false),
            todo: (0..num_pieces as u32).collect()
        }
    }

    pub fn complete(&mut self, piece_index: u32) {
        self.done.mark_piece(piece_index as usize).unwrap();
    }

    pub fn requeue(&mut self, piece_index: u32) {
        self.todo.insert(piece_index);
    }
}

impl PieceDownloadProgress {
    pub fn new(piece_size: usize) -> Self {
        PieceDownloadProgress { offset: 0, data: Vec::with_capacity(piece_size) }
    }

    pub fn remaining(&self) -> u32 {
        (self.data.capacity() as u32) - self.offset
    }

    pub fn get_next_block_size(&self) -> u32 {
        self.remaining().min(BLOCK_SIZE)
    }

    pub fn complete(&self) -> bool {
        self.get_next_block_size() == 0
    }

    pub fn add_block(&mut self, block: &[u8]) {
        self.data.extend_from_slice(block);
        self.offset += block.len() as u32;
    }
}

#[derive(Debug, Copy, Clone)]
enum State {
    Curious, // waiting to see what pieces peer offers (via BitField response)
    Interested, // confirmed peer offers all pieces, waiting for unchoke response
    NotInterested, // peer does not offer all pieces, quit
    Choked, // waiting for unchoke to be able to request pieces
    Unchoked, // able to request if interested
    
}

#[macro_export]
macro_rules! piece_filename {
    ($a:expr) => {
        format!("piece_{}.bin", $a)
    };
}

impl Downloader {
    pub async fn new(address: SocketAddrV4,
               info: Arc<FileDownloadInfo>,
               state: Arc<Mutex<FileDownloadState>>,
               dir: Arc<PathBuf>,
               pb: ProgressBar
               ) -> std::io::Result<Self> {
        info!("connecting to peer {} ...", address);
        let connection = match TcpStream::connect(address).await {
            Ok(c) => {
                info!("connected to peer {}", address);
                c
            },
            Err(e) => {
                error!("error connecting to peer {}: {:?}", address, e);
                return Err(e);
            }
        };

        Ok(Downloader {
            address,
            connection,
            info,
            shared_state: state,
            skip_set: HashSet::new(),
            state: State::Curious,
            dir,
            pb
        })
    }

    async fn get_message(&mut self) -> Result<Message, PeerError> {
        Message::read_message(&mut self.connection).await
    }

    pub async fn download_pieces(self: &mut Self) -> Result<(), PeerError> {
        info!("reaching out to handshake with peer {} (info hash = {})", self.address, to_string(&self.info.hash));
        handshake(&self.address, &mut self.connection, &self.info.hash).await?;

        let num_pieces = self.info.piece_hashes.len();
        let bitfield_len = (num_pieces + 7) / 8;
        let empty = vec![0u8; bitfield_len];

        info!("sending empty Bitfield message to peer {}", self.address);
        Message::send_bitfield(&mut self.connection, &empty).await?;
    
        self.state = State::Curious;

        loop {
            match self.state {
                State::Curious => {
                    info!("waiting for Bitfield response from peer {} ...", self.address);
                    let msg = self.get_message().await?;
                    if let Message::Bitfield { bitfield } = msg {
                        if bitfield.all() {
                            self.state = State::Interested;
                            Message::send_interested(&mut self.connection).await?;
                            info!("peer {} is a seed; interest expressed", self.address);
                        } else {
                            self.state = State::NotInterested;
                            info!("peer {} is not fully seeded; abandoning download", self.address);
                        }
                    }
                },
                State::Choked | State::Interested => {
                    let msg = self.get_message().await?;
                    if let Message::Unchoke = msg {
                        info!("peer {} sent Unchoke", self.address);
                        self.state = State::Unchoked;
                    }
                },
                State::Unchoked => {
                    if let Err(PeerError::Exhausted(_)) = self.try_download_piece().await {
                        self.state = State::NotInterested;
                    }
                },
                State::NotInterested => {
                    break;
                }
            }
        }

        Ok(())
    }

    async fn try_download_piece(&mut self) -> Result<(), PeerError> {
        let piece = {
            let mut guard = self.shared_state.lock().await;

            if let Some(&p) = guard.todo.iter().find(|&&p| !self.skip_set.contains(&p)) {
                guard.todo.remove(&p);
                p
            } else {
                info!("peer {} exhausted all pieces; exiting...", self.address);
                return Err(PeerError::Exhausted(self.address.to_string()));
            }
        };

        info!("peer {} selected piece {}", self.address, piece);
        let expected_hash = self.info.piece_hashes[piece as usize];
        info!("download of piece {} from peer {} starting, expecting hash {}", piece, self.address, to_string(&expected_hash));
        let result = Downloader::download_piece(&mut self.connection, piece, self.info.bytes_per_piece, &self.address).await;

        match result {
            Ok(data) => {
                let data_hash = sha1_hash(&data);
                info!("peer {} retrieved piece {} with SHA1 hash {}", self.address, piece, to_string(&data_hash));
                if data_hash == expected_hash {
                    let path = self.dir.join(piece_filename!(piece));
                    let path_str = path.to_string_lossy();
                    info!("peer {} writing piece {} to {}...", self.address, piece, path_str);
                    Downloader::save_piece(&path, &data)
                            .await
                            .map_err(|e| PeerError::DiskError(piece, e))?;
                    info!("peer {} wrote piece {} to {}", self.address, piece, path_str);
                    let mut guard = self.shared_state.lock().await;
                    self.pb.inc(self.info.bytes_per_piece as u64);
                    guard.complete(piece);
                } else {
                    error!("peer {} found hash of piece {} mismatches, adding piece to skip list and re-queueing for another peer", self.address, piece);
                    self.skip_set.insert(piece);
                    let mut guard = self.shared_state.lock().await;
                    guard.requeue(piece);
                }
            },
            Err(e) => {
                error!("peer {} took error during download: {:?}", self.address, e);
                self.skip_set.insert(piece);
                let mut guard = self.shared_state.lock().await;
                guard.requeue(piece);
                self.state = State::Choked;
            }
        }
        Ok(())
    }

    async fn download_piece(stream: &mut TcpStream, piece: u32, length: usize, address: &SocketAddrV4) -> Result<Box<[u8]>, PeerError> {
        let mut progress = PieceDownloadProgress::new(length);
        let mut choked = false;
        let mut request_size = 0u32;

        while !progress.complete() {
            if !choked {
                request_size = progress.get_next_block_size();
                info!("asking for {} bytes at offset {} for piece {} from peer {} ({} bytes remain)", request_size, progress.offset, piece, address, progress.remaining());
                Message::send_request(stream, piece, progress.offset, request_size).await?;
            }

            match Message::read_message(stream).await? {
                Message::Piece { index, begin, bytes} => {
                    info!("peer {} responsed with piece {} at offset {} with length {}", address, index, begin, bytes.len());
                    if index == piece && progress.offset == begin && bytes.len() as u32 == request_size {
                        progress.add_block(&bytes);
                    }
                },
                Message::Choke => {
                    info!("peer {} sent choke", address);
                    choked = true;
                }
                Message::Unchoke => {
                    info!("peer {} sent unchoke", address);
                    choked = false;
                }
                _ => (),
            }
        }

        info!("finished download of piece {} from peer {}", piece, address);

        Ok(progress.data.into_boxed_slice())
    }

    async fn save_piece(path: &Path, bytes: &[u8]) -> tokio::io::Result<()> {
        let mut file = File::create(path).await?;
        file.write_all(bytes).await
    }
}
