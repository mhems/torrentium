mod bencode;
mod torrent;

use bencode::BencodeParser;
use bencode::BencodeValue;
use torrent::TorrentFile;

fn main() {
    let contents : &[u8] = 
        "d8:announce41:http://bttracker.debian.org:6969/announce7:comment35:\"Debian CD from cdimage.debian.org\"13:creation datei1573903810e9:httpseedsl145:https://cdimage.debian.org/cdimage/release/10.2.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-10.2.0-amd64-netinst.iso145:https://cdimage.debian.org/cdimage/archive/10.2.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-10.2.0-amd64-netinst.isoe4:infod6:lengthi351272960e4:name31:debian-10.2.0-amd64-netinst.iso12:piece lengthi262144eee"
        .as_bytes();
    let mut parser: BencodeParser = BencodeParser::new(contents);
    let value: BencodeValue = parser.parse();
    println!("{}", &value);
    let torrent: TorrentFile = TorrentFile::from(value);
    dbg!(torrent);
}
