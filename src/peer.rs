mod client;
mod handshake;
mod message;
mod downloader;

use std::sync::Arc;

use crate::{metadata::file::{FileModeInfo::{Multiple, Single}, TorrentFile}, peer::downloader::{FileDownloadInfo, FileDownloadState}};
use crate::peer::{client::{ProtocolError, retrieve_peers}, downloader::Downloader};

pub use downloader::DownloadError;
use tokio::sync::Mutex;

const PEER_ID: &[u8; 20] = b"!MySuperCoolTorrent!";

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
        Single {filename, length, .. } => {
            let tracker_response = retrieve_peers(file, port).await
                .map_err(|_| DownloadError::RetrievalError)?;
            let n = num_seeds.min(tracker_response.peers.len());
            println!("using {} seeds (of {})", n, tracker_response.peers.len());

            let mut tasks = Vec::with_capacity(n);
            let filename_arc: Arc<String> = Arc::new(filename.into());

            let info = FileDownloadInfo::try_from(file)?;
            let info_arc = Arc::new(info);

            let state = FileDownloadState::new();
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
