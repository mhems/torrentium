use clap::Parser;
use torrent::{parse_torrent, download_torrent};

#[derive(Parser, Debug)]
#[command(name="torrentium", version)]
struct Args {
    #[arg(short, long, help="Print contents of torrent file")]
    inspect: bool,

    file: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let filename = args.file;

    if args.inspect {
        match parse_torrent(&filename) {
            Ok(torrent) => println!("Contents of {}:\n{}", &filename, torrent),
            Err(e) => println!("Unable to parse file: {:?}", e),
        }
    } else {
        match download_torrent(&filename).await {
            Ok(()) => println!("Successfully downloaded file(s) from {}!", &filename),
            Err(e) => println!("{:?}", e),
        }
    }
}
