use std::collections::HashSet;
use std::path::Path;
use std::net::SocketAddrV4;
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::metadata::file::{FileModeInfo, TorrentFile};
use crate::peer::Bitfield;
use crate::peer::handshake::handshake;
use crate::peer::{client::ProtocolError, message::{Message, MessageError}};
use crate::sha1::sha1_hash;

const BLOCK_SIZE: u32 = 16 * 1024;

#[derive(Debug)]
pub struct Downloader {
    pub address: SocketAddrV4,
    pub connection: Option<TcpStream>,
    info: Arc<FileDownloadInfo>,
    shared_state: Arc<Mutex<FileDownloadState>>,
    skip_set: HashSet<u32>,
    state: State,
}

#[derive(Debug, Clone)]
pub struct FileDownloadInfo {
    num_pieces: u64,
    bytes_per_piece: usize,
    length: u64,
    filename: Arc<String>,
    piece_hashes: Arc<Vec<[u8; 20]>>,
    hash: Arc<[u8; 20]>,
}

#[derive(Debug)]
pub struct FileDownloadState {
    pub(crate) done: Bitfield,
    pub(crate) todo: HashSet<u32>,
}

#[derive(Debug)]
struct PieceDownloadProgress {
    offset: u32,
    data: Vec<u8>,
}

#[derive(Debug)]
pub enum DownloadError {
    NotConnected,
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
    Message
}

impl TryFrom<&TorrentFile> for FileDownloadInfo {
    type Error = DownloadError;
    fn try_from(file: &TorrentFile) -> Result<Self, DownloadError> {
        match file.info.as_ref() {
            FileModeInfo::Single { filename, .. } => {
                Ok(FileDownloadInfo {
                    num_pieces: file.num_pieces as u64,
                    bytes_per_piece: file.num_bytes_per_piece as usize,
                    length: file.total_num_bytes,
                    filename: Arc::new(filename.clone()),
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

#[derive(Debug)]
enum State {
    Curious, // waiting to see what pieces peer offers (via BitField response)
    Interested, // confirmed peer offers all pieces, waiting for unchoke response
    NotInterested, // peer does not offer all pieces, quit
    Choked, // waiting for unchoke to be able to request pieces
    Unchoked, // able to request if interested
    
}

impl Downloader {
    pub fn new(address: SocketAddrV4,
               info: Arc<FileDownloadInfo>,
               state: Arc<Mutex<FileDownloadState>>
               ) -> Self {
        Downloader { address, connection: None, info, shared_state: state, skip_set: HashSet::new(), state: State::Curious }
    }

    async fn connect(&mut self) -> std::io::Result<()> {
        self.connection = Some(TcpStream::connect(self.address).await?);
        Ok(())
    }

    async fn handshake(&mut self) -> Result<(), DownloadError> {
        if let Some(stream) = self.connection.as_mut() {
            handshake(stream, &self.info.hash).await
        } else {
            Err(DownloadError::NotConnected)
        }
    }

    async fn get_message(&mut self) -> Result<Message, ProtocolError> {
        let stream= self.connection.as_mut().unwrap();
        Message::read_message(stream).await.map_err(|e| ProtocolError::ReadError(e))
    }

    async fn try_download_piece(&mut self) -> Result<(), ProtocolError> {
        let piece = {
            let mut guard = self.shared_state.lock().await;
            println!("[{}]: {:.4}% to go", self.address, (guard.todo.len() as f32)*100.0 / (self.info.num_pieces as f32));
            let mut candidates = guard.todo.difference(&self.skip_set);
            if let Some(value) = candidates.next().cloned() {
                guard.todo.remove(&value);
                value
            } else {
                println!("[{}]: exhausted", self.address);
                return Err(ProtocolError::Exhausted);
            }
        };

        let result = {
            let stream= self.connection.as_mut().unwrap();
            Downloader::download_piece(stream, piece, self.info.bytes_per_piece)
                .await
                .map_err(|e| ProtocolError::ReadError(e))
        };

        match result {
            Ok(data) => {
                let data_hash = sha1_hash(&data);
                if data_hash == self.info.piece_hashes[piece as usize] {
                    println!(">>>> [{}]: DOWNLOADED ALL OF PIECE {} AND IT MATCHES!", self.address, piece);
                    self.save_piece(piece, &data).await.map_err(|e| ProtocolError::DiskError(e))?;
                    {
                        let mut guard = self.shared_state.lock().await;
                        if guard.done.has_piece(piece as usize) {
                            println!("collision on {}", piece);
                            std::process::exit(1);
                        }
                        guard.complete(piece);
                    }
                } else {
                    println!(">>>> [{}]: DOWNLOADED ALL OF PIECE {} BUT IT DOESN'T MATCH!", self.address, piece);
                    self.skip_set.insert(piece);
                    self.shared_state.lock().await.requeue(piece);
                }
            },
            Err(e) => {
                println!(">>>> [{}] took error {:?}", self.address, e);
                self.skip_set.insert(piece);
                self.shared_state.lock().await.requeue(piece);
                self.state = State::Choked;
            }
        }
        Ok(())
    }

    pub async fn download_file(self: &mut Self) -> Result<(), ProtocolError> {
        self.connect().await.map_err(|e| ProtocolError::ConnectionError(e))?;
        println!("...connected to {}!", self.address);

        self.handshake().await.map_err(|e| ProtocolError::HandshakeError(e))?;
        println!("...handshaked with {}!", self.address);

        let num_pieces = self.info.piece_hashes.len();
        let bitfield_len = (num_pieces + 7) / 8;
        let empty = vec![0u8; bitfield_len];

        {
            let stream= self.connection.as_mut().unwrap();
            Message::send_bitfield(stream, &empty).await.map_err(|e| ProtocolError::WriteError(e))?;
        }

        self.state = State::Curious;
        let mut interested = false;

        loop {
            match self.state {
                State::Curious => {
                    println!("[{}]: waiting for BitField", self.address);
                    let msg = self.get_message().await?;
                    if let Message::Bitfield { bitfield } = msg {
                        if bitfield.all() {
                            let stream= self.connection.as_mut().unwrap();
                            self.state = State::Interested;
                            interested = true;
                            Message::send_interested(stream).await.map_err(|e| ProtocolError::WriteError(e))?;
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

    async fn save_piece(&self, piece: u32, bytes: &[u8]) -> tokio::io::Result<()> {
        let path = Path::new(self.info.filename.as_ref());
        let base = path.file_stem().unwrap().to_string_lossy();
        let path = format!("{}.{}.bin", base, piece.to_string());
        let mut file = File::create(path).await?;
        file.write_all(bytes).await?;
        Ok(())
    }
}
