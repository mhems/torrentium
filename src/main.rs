use clap::Parser;
use torrent::download_torrent;

#[derive(Parser, Debug)]
#[command(name="torrentium")]
struct Args {
    #[arg(short, long)]
    file: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let filename = args.file;

    match download_torrent(&filename).await {
        Ok(()) => println!("Successfully downloaded file(s) from {}!", &filename),
        Err(e) => println!("{:?}", e),
    }
}
