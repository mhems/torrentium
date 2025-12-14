use clap::Parser;
use tracing_subscriber::fmt::time::LocalTime;
use tracing_appender::non_blocking;
use time::macros::format_description;

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

    let file_appender = tracing_appender::rolling::never("logs", "torrent.log");
    let (non_blocking, _guard) = non_blocking(file_appender);
    let timer = LocalTime::new(
        format_description!("[month]/[day]/[year] [hour repr:24]:[minute]:[second].[subsecond digits:4]"));
    tracing_subscriber::fmt().with_writer(non_blocking).with_ansi(false).with_timer(timer).init();
    //tracing_subscriber::fmt().with_timer(timer).init();

    if args.inspect {
        match parse_torrent(&filename) {
            Ok(torrent) => println!("Contents of {}:\n{}", &filename, torrent),
            Err(e) => println!("Unable to parse file: {e:?}"),
        }
    } else {
        match download_torrent(&filename).await {
            Ok(()) => println!("Successfully downloaded file(s) from {}!", &filename),
            Err(e) => println!("{e:?}"),
        }
    }
}
