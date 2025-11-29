use std::panic;
use std::collections::HashMap;

use crate::bencode::BencodeValue;

#[derive(Debug)]
pub struct TorrentFile {
    announce: String,
    announce_list: Vec<Vec<String>>,
    creation_date: Option<u64>,
    comment: Option<String>,
    created_by: Option<String>,
    encoding: Option<String>,

    info: FileModeInfo,
    piece_length: u64,
    pieces: Vec<[u8; 20]>,
    private: bool,
}

#[derive(Debug)]
pub enum FileModeInfo {
    Single {filename: String, length: u64, md5sum: Option<[u8; 32]>},
    Multiple {directory: String, files: Vec<MultiFileInfo>},
}

#[derive(Debug)]
pub struct MultiFileInfo {
    length: u64,
    md5sum: Option<[u8; 32]>,
    path: Vec<String>,
}

impl From<BencodeValue> for TorrentFile {
    fn from(value: BencodeValue) -> Self {
        match value {
            BencodeValue::Dictionary(items ) => {
                match items.get("info") {
                    Some(v) => {
                        match &**v {
                            BencodeValue::Dictionary(info_items ) => {
                                let mut file: TorrentFile = TorrentFile::new(info_items.contains_key("files"));
                                file.extract(items);
                                return file;
                            },
                            _ => panic!("the value associated with the key 'info' must be a dictionary")
                        }
                    },
                    None => panic!("torrent file must contain a dictionary with the key 'info'")
                };
            },
            _ => panic!("a torrent file must be encoded as a dictionary"),
        }
    }
}

fn extract_string(items: &HashMap<String, Box<BencodeValue>>, name: String, mandatory: bool) -> Option<String> {
    match items.get(&name) {
        Some(v) => {
            match &**v {
                BencodeValue::String(text) => Some(text.to_owned()),
                _ => panic!("expected '{}' to map to a string", name),
            }
        },
        None => if mandatory { panic!("expected key '{}'", name)} else { None },
    }
}

fn extract_uint(items: &HashMap<String, Box<BencodeValue>>, name: String, mandatory: bool) -> Option<u64> {
    match items.get(&name) {
        Some(v) => {
            match &**v {
                BencodeValue::Integer(num) => {
                    if *num < 0 {
                        panic!("integer must be non-negative")
                    }
                    else {
                        Some(u64::from(num.unsigned_abs()))
                    }
                },
                _ => panic!("expected '{}' to map to an integer", name),
            }
        },
        None => if mandatory { panic!("expected key '{}'", name)} else { None },
    }
}

fn extract_list_of_string(value: Option<&Box<BencodeValue>>) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    match value {
        Some(v) => {
            match &**v {
                BencodeValue::List(elements) => {
                    for element in elements {
                        match element {
                            BencodeValue::String(text) => {
                                list.push(text.to_owned());
                            },
                            _ => panic!("list can only strings"),
                        }
                    }
                },
                _ => panic!("'announce-list' must map to a list"),
            }
        },
        None => (),
    }
    return list;
}

fn extract_md5sum(value: Option<&Box<BencodeValue>>) -> Option<[u8; 32]> {
    return match value {
        Some(v) => {
            match &**v {
                BencodeValue::String(array) => {
                        let length = array.len();
                        if length != 32 {
                            panic!("'md5sum' must have a length of 32")
                        }
                        let bytes: [u8; 32] = array.as_bytes().try_into().expect("expected md5sum to be 32 bytes");
                        return Some(bytes);
                },
                _ => panic!("'md5sum' key must map to a string value"),
            };
        },
        None => None,
    };
}

fn extract_multi_file_info(items: &HashMap<String, Box<BencodeValue>>) -> MultiFileInfo {
    let length = match extract_uint(items, String::from("length"), true) {
        Some(v) => v,
        None => panic!("file must contain a 'length' key"),
    };
    let path: Vec<String> = extract_list_of_string(items.get(&String::from("path")));
    let md5sum = extract_md5sum(items.get(&String::from("md5sum")));
    return MultiFileInfo { length: length, md5sum: md5sum, path: path }    
}

impl TorrentFile {
    pub fn new(multi: bool) -> Self {
        let info : FileModeInfo = if multi {
            FileModeInfo::Multiple { directory: String::new(), files: Vec::new() }
        } else {
            FileModeInfo::Single { filename: String::new(), length: 0, md5sum: None }
        };
        return TorrentFile {
            announce: String::new(),
            announce_list: Vec::new(),
            creation_date: None,
            comment: None,
            created_by: None,
            encoding: None,
            piece_length: 0,
            pieces: Vec::new(),
            private: false,
            info: info
        }
    }

    fn extract(&mut self, mut items: HashMap<String, Box<BencodeValue>>) {
        self.announce = match extract_string(&items, String::from("announce"), true) {
            Some(value) => value,
            None => panic!("a 'announce' key is required")
        };
        self.extract_announce_list(items.remove(&String::from("announce-list")));
        self.creation_date = extract_uint(&items, String::from("creation date"), false);
        self.comment = extract_string(&items, String::from("comment"), false);
        self.created_by = extract_string(&items, String::from("created by"), false);
        self.encoding = extract_string(&items, String::from("encoding"), false);

        let info = match items.get("info") {
            Some(v) => v,
            None => panic!("torrent file must contain 'info' key"),
        };
        let info_items = match &**info {
            BencodeValue::Dictionary(items) => items,
            _ => panic!("'info' must map to a dictionary"),
        };
        self.piece_length = match extract_uint(&info_items, String::from("piece length"), true) {
            Some(value) => value,
            None => panic!("a piece length' key is required")
        };
        self.private = match extract_uint(&info_items, String::from("private"), false) {
            Some(i) => {
                match i {
                    0 => false,
                    1 => true,
                    _ => panic!("private must map to either '0' or '1'"),
                }
            },
            None => false,
        };
        self.extract_pieces(info_items.get(&String::from("pieces")));
        let name = match extract_string(&info_items, String::from("name"), true) {
            Some(text) => text,
            None => panic!("info dictionary must contain a 'name' key"),
        };
        match self.info {
            FileModeInfo::Single{..} => {
                let length = match extract_uint(&info_items, String::from("length"), true) {
                    Some(num) => num,
                    None => panic!("info dictionary must contain a 'length' key"),
                };
                let md5sum = extract_md5sum(info_items.get(&String::from("md5sum")));
                self.info = FileModeInfo::Single { filename: name, length: length, md5sum: md5sum }
            },
            FileModeInfo::Multiple {..} => {
                let mut files = Vec::new();
                match info_items.get(&String::from("files")) {
                    Some(v) => {
                        match &**v {
                            BencodeValue::List(elements) => {
                                for element in elements {
                                    match element {
                                        BencodeValue::Dictionary(items) => {
                                            files.push(extract_multi_file_info(items));
                                        },
                                        _ => panic!("'files' key must map to a list of dictionaries")
                                    }
                                }
                            },
                            _ => panic!("'files' key must map to a list")
                        }
                    },
                    None => panic!("info dictionary must contain a 'files' key")
                }
                self.info = FileModeInfo::Multiple { directory: name, files: files }
            },
        }
    }

    fn extract_announce_list(&mut self, value: Option<Box<BencodeValue>>) {
        match value {
            Some(v) => {
                match *v {
                    BencodeValue::List(elements) => {
                        for element in elements {
                            let inner_list = extract_list_of_string(Some(&Box::new(element)));
                            self.announce_list.push(inner_list);
                        }
                    },
                    _ => panic!("'announce-list' must map to a list"),
                }
            },
            None => (),
        }
    }

    fn extract_pieces(&mut self, value: Option<&Box<BencodeValue>>) {
        match value {
            Some(v) => {
                match &**v {
                    BencodeValue::String(s) => {
                        let length = s.len();
                        if length % 20 != 0 {
                            panic!("'pieces' must have a length that is a multiple of 20")
                        }
                        let mut i: usize = 0;
                        let bytes= s.as_bytes();
                        while i < length {
                            let slice: [u8;20] = bytes[i..(i+20)].try_into().expect("expected 20 bytes");
                            self.pieces.push(slice);
                            i += 20;
                        }
                    },
                    _ => panic!("'pieces' key must map to a string value")
                }
            },
            None => (), //panic!("info must have a 'pieces' key"),
        }
    }

}