mod bencode;
mod torrent;
mod sha1;

use torrent::TorrentFile;

use bencode::BencodeValue;

fn main() {
    let contents : &[u8] = 
        "d8:announce41:http://bttracker.debian.org:6969/announce7:comment35:\"Debian CD from cdimage.debian.org\"13:creation datei1573903810e9:httpseedsl145:https://cdimage.debian.org/cdimage/release/10.2.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-10.2.0-amd64-netinst.iso145:https://cdimage.debian.org/cdimage/archive/10.2.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-10.2.0-amd64-netinst.isoe4:infod6:lengthi351272960e4:name31:debian-10.2.0-amd64-netinst.iso12:piece lengthi262144eee"
        .as_bytes();
    println!("\nparsing Bencoded value...");
    let result = BencodeValue::try_from(contents);
    match &result {
        Ok(value) => {
            println!("parsed Bencoded value:\n{}", &value);

            let encoded: Vec<u8> = Vec::from(value);
            assert_eq!(contents, encoded);

            println!("\nconverting Bencoded value to torrent file...");
            match TorrentFile::try_from(value) {
                Ok(torrent) => {
                    println!("torrent file:\n{}", &torrent)
                },
                Err(e) => println!("{:?}", e)
            }
        },
        Err(e) => println!("{:?}", e),
    };
}
