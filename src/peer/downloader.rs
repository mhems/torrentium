use std::collections::HashSet;
use std::path::Path;
use std::net::SocketAddrV4;
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::metadata::file::{FileModeInfo, TorrentFile};
use crate::peer::handshake::handshake;
use crate::peer::{client::ProtocolError, message::{Message, MessageError}};
use crate::sha1::sha1_hash;

const BLOCK_SIZE: u32 = 16 * 1024;

#[derive(Debug)]
pub struct Downloader {
    pub address: SocketAddrV4,
    pub connection: Option<TcpStream>,
    info: Arc<FileDownloadInfo>,
    state: Arc<Mutex<FileDownloadState>>,
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
    done: HashSet<u32>
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
    pub fn new() -> Self {
        FileDownloadState { done: HashSet::new() }
    }

    pub fn mark(&mut self, piece_index: u32) {
        self.done.insert(piece_index);
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

impl Downloader {
    pub fn new(address: SocketAddrV4,
               info: Arc<FileDownloadInfo>,
               state: Arc<Mutex<FileDownloadState>>
               ) -> Self {
        Downloader { address, connection: None, info, state }
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

    pub async fn download_file(self: &mut Self) -> Result<(), ProtocolError> {
        self.connect().await.map_err(|e| ProtocolError::ConnectionError(e))?;
        println!("...connected to {}!", self.address);

        self.handshake().await.map_err(|e| ProtocolError::HandshakeError(e))?;
        println!("...handshaked with {}!", self.address);

        let num_pieces = self.info.piece_hashes.len();
        let bitfield_len = (num_pieces + 7) / 8;
        let empty = vec![0u8; bitfield_len];

        {
            let mut stream= self.connection.as_mut().unwrap();
            Message::send_bitfield(stream, &empty).await.map_err(|e| ProtocolError::WriteError(e))?;
        }

        let mut done = false;
        let mut choked = true;

        while !done {
            println!("[{}]: waiting for message", self.address);
            let msg = {
                let mut stream= self.connection.as_mut().unwrap();
                Message::read_message(stream).await.map_err(|e| ProtocolError::ReadError(e))?
            };

            match msg {
                Message::Choke => {
                    println!("[{}]: choke", self.address);
                    choked = true;
                },
                Message::Unchoke => {
                    println!("[{}]: unchoke", self.address);
                    choked = false;

                    // TODO pick a piece and pop it from candidates
                    let piece: u32 = 0;

                    let result = {
                        let mut stream= self.connection.as_mut().unwrap();
                        Downloader::download_piece(stream, piece, self.info.bytes_per_piece)
                        .await
                        .map_err(|e| ProtocolError::ReadError(e))
                    };
                    match result {
                        Ok(data) => {
                            let data_hash = sha1_hash(&data);
                            if data_hash == self.info.piece_hashes[piece as usize] {
                                println!(">>>> DOWNLOADED ALL OF PIECE {} AND IT MATCHES!", piece);
                                self.save_piece(piece, &data).await.map_err(|e| ProtocolError::DiskError(e))?;
                                self.state.lock().await.mark(piece);
                            } else {
                                // TODO put back in candidates
                            }
                        },
                        Err(e) => {
                            // TODO put back in candidates
                        }
                    }
                },
                Message::Bitfield { bitmap } => {
                    println!("[{}]: Bitfield: len = {}", self.address, bitmap.len());
                    let num_pieces = bitmap.len() * 8;
                    let all_set = bitmap.iter().all(|&b| b == 0xff);
                    if num_pieces == self.info.piece_hashes.len() && all_set {
                        let mut stream= self.connection.as_mut().unwrap();
                        Message::send_interested(&mut stream).await.map_err(|e| ProtocolError::WriteError(e))?;
                        println!("[{}]: interest expressed", self.address);
                    } else {
                        println!("[{}]: not fully seeded, abandoning", self.address);
                        break;
                    }
                },
                _ => (), //println!("[{}]: got msg: {:?}", self.address, msg),
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

            let msg = Message::read_message(stream).await?;

            match msg {
                Message::Piece { index, begin, bytes} => {
                    if index == piece && progress.offset == begin && bytes.len() as u32 == request_size {
                        progress.add_block(&bytes);
                        print!(".");
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
