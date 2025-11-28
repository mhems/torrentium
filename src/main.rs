mod bencode;

use bencode::BencodeParser;
use bencode::BencodeValue;

fn main() {
    let contents : &[u8] = "li6e5:world3:byed4:wikai42e4:wiki7:bencodeee".as_bytes();
    let mut parser: BencodeParser = BencodeParser::new(contents);
    let value: BencodeValue = parser.parse();
    println!("{}", &value);
}
