use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use std::path::{Path, PathBuf};

use crate::metadata::file::{FileModeInfo, TorrentFile};
use crate::peer::Bitfield;
use crate::peer::handshake::handshake;
use crate::peer::{client::ProtocolError, message::{Message, MessageError}};
use crate::util::sha1::sha1_hash;

const BLOCK_SIZE: u32 = 16 * 1024;

#[derive(Debug)]
pub struct Downloader {
    pub address: SocketAddrV4,
    connection: TcpStream,
    info: Arc<FileDownloadInfo>,
    shared_state: Arc<Mutex<FileDownloadState>>,
    skip_set: HashSet<u32>,
    state: State,
    basename: Arc<String>,
    dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FileDownloadInfo {
    num_pieces: u64,
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

#[derive(Debug)]
pub enum DownloadError {
    CannotDownloadMultipleFiles,
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
    RetrievalError,
    Message,
    FileSystemError(std::io::Error)
}

impl TryFrom<&TorrentFile> for FileDownloadInfo {
    type Error = DownloadError;
    fn try_from(file: &TorrentFile) -> Result<Self, DownloadError> {
        match &file.info {
            FileModeInfo::Single { .. } => {
                Ok(FileDownloadInfo {
                    num_pieces: file.num_pieces as u64,
                    bytes_per_piece: file.num_bytes_per_piece as usize,
                    piece_hashes: file.piece_hashes.clone(),
                    hash: file.hash.clone(),
                })
            },
            FileModeInfo::Multiple { .. } => Err(DownloadError::CannotDownloadMultipleFiles)
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
        self.done.mark_piece(piece_index as usize);
    }

    pub fn requeue(&mut self, piece_index: u32) {
        self.todo.insert(piece_index);
    }
}

impl PieceDownloadProgress {
    pub fn new(piece_size: usize) -> Self {
        PieceDownloadProgress { offset: 0, data: Vec::with_capacity(piece_size) }
    }

    pub fn get_next_block_size(&self) -> u32 {
        let remainder = (self.data.capacity() as u32) - self.offset;
        remainder.min(BLOCK_SIZE)
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

impl Downloader {
    pub async fn new(address: SocketAddrV4,
               info: Arc<FileDownloadInfo>,
               state: Arc<Mutex<FileDownloadState>>,
               dir: PathBuf,
               basename: Arc<String>
               ) -> std::io::Result<Self> {
        Ok(Downloader {
            address,
            connection: TcpStream::connect(address).await?,
            info,
            shared_state: state,
            skip_set: HashSet::new(),
            state: State::Curious,
            dir,
            basename
        })
    }

    async fn handshake(&mut self) -> Result<(), DownloadError> {
        handshake( &mut self.connection, &self.info.hash).await
    }

    async fn get_message(&mut self) -> Result<Message, ProtocolError> {
        Message::read_message(&mut self.connection)
            .await
            .map_err(|e| ProtocolError::ReadError(e))
    }

    pub async fn download_file(self: &mut Self) -> Result<(), ProtocolError> {
        self.handshake().await.map_err(|e| ProtocolError::HandshakeError(e))?;
        println!("...handshaked with {}!", self.address);

        let num_pieces = self.info.piece_hashes.len();
        let bitfield_len = (num_pieces + 7) / 8;
        let empty = vec![0u8; bitfield_len];

        Message::send_bitfield(&mut self.connection, &empty)
            .await
            .map_err(|e| ProtocolError::WriteError(e))?;
    
        self.state = State::Curious;

        loop {
            match self.state {
                State::Curious => {
                    println!("[{}]: waiting for BitField", self.address);
                    let msg = self.get_message().await?;
                    if let Message::Bitfield { bitfield } = msg {
                        if bitfield.all() {
                            self.state = State::Interested;
                            Message::send_interested(&mut self.connection)
                                .await
                                .map_err(|e| ProtocolError::WriteError(e))?;
                            println!("[{}]: interest expressed", self.address);
                        } else {
                            self.state = State::NotInterested;
                            println!("[{}]: not fully seeded, abandoning", self.address);
                        }
                    };
                },
                State::Choked | State::Interested => {
                    let msg = self.get_message().await?;
                    if let Message::Unchoke = msg {
                        self.state = State::Unchoked;
                    }
                },
                State::Unchoked => {
                    if let Err(ProtocolError::Exhausted) = self.try_download_piece().await {
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

    async fn try_download_piece(&mut self) -> Result<(), ProtocolError> {
        let piece = {
            let mut guard = self.shared_state.lock().await;
            println!("[{}]: {:.4}% to go", self.address, (guard.todo.len() as f32) * 100.0 / (self.info.num_pieces as f32));

            if let Some(&p) = guard.todo.iter().find(|&&p| !self.skip_set.contains(&p)) {
                guard.todo.remove(&p);
                p
            } else {
                println!("[{}]: exhausted", self.address);
                return Err(ProtocolError::Exhausted);
            }
        };

        let result = Downloader::download_piece(&mut self.connection, piece, self.info.bytes_per_piece)
                .await
                .map_err(|e| ProtocolError::ReadError(e));

        match result {
            Ok(data) => {
                let data_hash = sha1_hash(&data);
                if data_hash == self.info.piece_hashes[piece as usize] {
                    Downloader::save_piece(&self.dir, &self.basename, piece, &data)
                            .await
                            .map_err(|e| ProtocolError::DiskError(e))?;
                    let mut guard = self.shared_state.lock().await;
                    guard.complete(piece);
                } else {
                    println!(">>>> [{}]: DOWNLOADED ALL OF PIECE {} BUT HASHES MIS-MATCH!", self.address, piece);
                    self.skip_set.insert(piece);
                    let mut guard = self.shared_state.lock().await;
                    guard.requeue(piece);
                }
            },
            Err(e) => {
                println!(">>>> [{}] took error {:?}", self.address, e);
                self.skip_set.insert(piece);
                let mut guard = self.shared_state.lock().await;
                guard.requeue(piece);
                self.state = State::Choked;
            }
        }
        Ok(())
    }

    async fn download_piece(stream: &mut TcpStream, piece: u32, length: usize) -> Result<Vec<u8>, MessageError> {
        let mut progress = PieceDownloadProgress::new(length);
        let mut choked = false;
        let mut request_size = 0u32;

        while !progress.complete() {

            if !choked {
                request_size = progress.get_next_block_size();
                Message::send_request(stream, piece, progress.offset, request_size).await?;
            }

            match Message::read_message(stream).await? {
                Message::Piece { index, begin, bytes} => {
                    if index == piece && progress.offset == begin && bytes.len() as u32 == request_size {
                        progress.add_block(&bytes);
                    }
                },
                Message::Choke => choked = true,
                Message::Unchoke => choked = false,
                _ => (),
            }
        }

        Ok(progress.data)
    }

    async fn save_piece(dir: &Path, basename: &str, piece: u32, bytes: &[u8]) -> tokio::io::Result<()> {
        let path = dir.join(format!("{}.{}.bin", basename, piece));
        let mut file = File::create(path).await?;
        file.write_all(bytes).await?;
        Ok(())
    }
}
