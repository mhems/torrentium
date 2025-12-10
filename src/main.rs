use std::fs;

use clap::Parser;

use torrent::metadata::bencode::BencodeValue;
use torrent::metadata::file::TorrentFile;
use torrent::peer::download_file;

#[derive(Parser, Debug)]
#[command(name="torrentium")]
struct Args {
    #[arg(short, long)]
    file: String,

    #[arg(short, long)]
    port: u16,

    #[arg(short, long)]
    num_seeds: usize
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let filename = args.file;

    match fs::read(&filename) {
        Ok(contents) => {
            match BencodeValue::try_from(contents.as_slice()) {
                Ok(value) => {
                    match TorrentFile::try_from(&value) {
                        Ok(torrent) => {
                            println!("contents of {}:\n{}", &filename, &torrent);

                            if let Err(e) = download_file(&torrent, args.port, args.num_seeds).await {
                                println!("error downloading file {}: {:?}", &filename, e);
                            }
                        },
                        Err(e) => {
                            println!("unrecognized Torrent file {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    println!("unable to parse into Bencoded value: {:?}", e)
                }
            }
        },
        Err(e) => println!("unable to read file {}: {:?}", &filename, e),
    }
}
