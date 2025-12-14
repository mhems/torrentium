use std::path::Path;

use crate::metadata::file::TorrentFile;

mod metadata;
mod peer;
mod util;

pub use peer::Bitfield;
pub use peer::message::Message;

const PEER_ID: &[u8; 20] = b"!MySuperCoolTorrent!";

pub fn parse_torrent<P: AsRef<Path>>(path: P) -> std::result::Result<TorrentFile, Box<dyn std::error::Error>> {
    let path = path.as_ref();
    let torrent_file: TorrentFile = TorrentFile::new(path).map_err(|e| Box::new(e))?;
    Ok(torrent_file)
}

pub async fn download_torrent<P: AsRef<Path>>(path: P) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let torrent_file: TorrentFile = parse_torrent(path)?;
    let response = torrent_file.retrieve_peers().await?;
    torrent_file.download(&response.peers).await
}
