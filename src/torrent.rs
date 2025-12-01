use std::collections::BTreeMap;
use std::fmt;

use crate::sha1;
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
    num_bytes_per_piece: u64,
    piece_hashes: Vec<[u8; 20]>,
    hash: [u8; 20],
    private: bool,
}

#[derive(Debug)]
pub enum FileModeInfo {
    Single {filename: String, length: u64, md5sum: Option<[u8; 16]>},
    Multiple {directory: String, files: Vec<MultiFileInfo>},
}

#[derive(Debug)]
pub struct MultiFileInfo {
    length: u64,
    md5sum: Option<[u8; 16]>,
    path: Vec<String>,
}

#[derive(Debug)]
pub enum TorrentError {
    KeyDoesNotMapToString(&'static str),
    MissingRequiredKey(&'static str),
    KeyDoesNotMapToInteger(&'static str),
    NegativeInteger(i64),
    KeyDoesNotMapToDictionary(&'static str),
    FileIsNotDictionary,
    KeyDoesNotMapToList(&'static str),
    InvalidMd5Length(usize),
    InvalidPrivateValue(u64),
    KeyDoesNotMapToListOfStrings(&'static str),
    KeyMapsToAnEmptyList(&'static str),
    InvalidNumberOfPieces(usize),
    InvalidAnnounceListElement,
    InvalidString(Vec<u8>),
}

type Result<T> = std::result::Result<T, TorrentError>;

impl fmt::Display for FileModeInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FileModeInfo::Single{filename, length, ..} => {
                write!(f, "{} ({} bytes)", filename, length)
            },
            FileModeInfo::Multiple { directory, files } => {
                let file_list = files
                    .iter()
                    .map(|i| format!("{} ({} bytes)", i.path.join("/"), i.length))
                    .collect::<Vec<_>>()
                    .join(", ");

                write!(f, "[{}] -> {}/", file_list, directory)
            }
        }
    }
}

impl fmt::Display for TorrentFile {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "announce: {}", self.announce)?;
        writeln!(f, "announce list: [{}]", self.announce_list
            .iter()
            .map(|v| format!("[{}]", v.join(", ")))
            .collect::<Vec<_>>()
            .join(", "))?;
        if let Some(date) = &self.creation_date {
            writeln!(f, "created: {} seconds since epoch", date)?;
        }
        if let Some(text) = &self.comment {
            writeln!(f, "comment: {}", text)?;
        }
        if let Some(author) = &self.created_by {
            writeln!(f, "created by: {}", author)?;
        }
        if let Some(e) = &self.encoding {
            writeln!(f, "encoding: {}", e)?;
        }
        writeln!(f, "private: {}", self.private)?;
        let num_pieces = (self.piece_hashes.len() / 20) as u64;
        writeln!(f, "size: {} bytes ({} pieces of {} bytes each)", self.num_bytes_per_piece * num_pieces, num_pieces, self.num_bytes_per_piece)?;
        writeln!(f, "file(s): {}", self.info)
    }
}

const ANNOUNCE: &[u8] = b"announce";
const ANNOUNCE_LIST: &[u8] = b"announce-list";
const CREATION_DATE: &[u8] = b"creation date";
const COMMENT: &[u8] = b"comment";
const CREATED_BY: &[u8] = b"created by";
const ENCODING: &[u8] = b"encoding";
const INFO: &[u8] = b"info";
const PIECE_LENGTH: &[u8] = b"piece length";
const PIECES: &[u8] = b"pieces";
const PRIVATE: &[u8] = b"private";
const NAME: &[u8] = b"name";
const LENGTH: &[u8] = b"length";
const MD5SUM: &[u8] = b"md5sum";
const FILES: &[u8] = b"files";
const PATH: &[u8] = b"path";

impl TryFrom<&BencodeValue> for TorrentFile {
    type Error = TorrentError;

    fn try_from(value: &BencodeValue) -> Result<Self> {
        match value {
            BencodeValue::Dictionary(items ) => {
                match items.get(INFO) {
                    Some(v) => {
                        match v {
                            BencodeValue::Dictionary(info_items ) => {
                                let mut file: TorrentFile = TorrentFile::new(info_items.contains_key(FILES));
                                file.extract(items, info_items)?;
                                return Ok(file);
                            },
                            _ => Err(TorrentError::KeyDoesNotMapToDictionary("info"))
                        }
                    },
                    None => Err(TorrentError::MissingRequiredKey("info"))
                }
            },
            _ => Err(TorrentError::FileIsNotDictionary)
        }
    }
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
            num_bytes_per_piece: 0,
            piece_hashes: Vec::new(),
            hash: [0; 20],
            private: false,
            info: info
        }
    }

    fn extract(&mut self, items: &BTreeMap<Vec<u8>, BencodeValue>, info_items: &BTreeMap<Vec<u8>, BencodeValue>) -> Result<()> {
        self.announce = Self::extract_string(items.get(ANNOUNCE), "announce", true)?.unwrap();
        self.extract_announce_list(items.get(ANNOUNCE_LIST))?;
        self.creation_date = Self::extract_uint(items.get(CREATION_DATE), "creation date", false)?;
        self.comment = Self::extract_string(items.get(COMMENT), "comment", false)?;
        self.created_by = Self::extract_string(items.get(CREATED_BY), "created by", false)?;
        self.encoding = Self::extract_string(items.get(ENCODING), "encoding", false)?;
        self.num_bytes_per_piece = Self::extract_uint(info_items.get(PIECE_LENGTH), "piece length", true)?.unwrap();
        let private = Self::extract_uint(info_items.get(PRIVATE), "private", false)?;
        if let Some(v) = private {
            if v <= 1 {
                self.private = if v == 0 { true } else { false };
            } else {
                return Err(TorrentError::InvalidPrivateValue(v));
            }
        } else {
            self.private = false;
        }
        self.extract_pieces(info_items.get(PIECES))?;
        self.hash = sha1::sha1_hash(Vec::from(items.get(INFO).unwrap()).as_slice());
        let name = Self::extract_string(info_items.get(NAME), "name", true)?.unwrap();
        match &self.info {
            FileModeInfo::Single{..} => {
                let length = Self::extract_uint(info_items.get(LENGTH), "length", true)?.unwrap();
                let md5sum = Self::extract_md5sum(info_items.get(MD5SUM))?;
                self.info = FileModeInfo::Single { filename: name, length: length, md5sum: md5sum };
            },
            FileModeInfo::Multiple {..} => {
                let mut files = Vec::new();
                match info_items.get(FILES) {
                    Some(v) => {
                        match v {
                            BencodeValue::List(elements) => {
                                for element in elements {
                                    match element {
                                        BencodeValue::Dictionary(items) => {
                                            files.push(Self::extract_multi_file_info(items)?);
                                        },
                                        _ => return Err(TorrentError::KeyDoesNotMapToListOfStrings("files"))
                                    }
                                }
                            },
                            _ => return Err(TorrentError::KeyDoesNotMapToListOfStrings("files"))
                        }
                    },
                    None => return Err(TorrentError::MissingRequiredKey("files"))
                }
                if files.is_empty() {
                    return Err(TorrentError::KeyMapsToAnEmptyList("files"))
                }
                self.info = FileModeInfo::Multiple { directory: name, files: files };
            },
        }
        Ok(())
    }

    fn convert_string(value: &BencodeValue) -> Option<String> {
        match value {
            BencodeValue::ByteString(text) => {
                std::str::from_utf8(text).map(str::to_owned).ok()
            },
            _ => None,
        }
    }

    fn extract_string(value: Option<&BencodeValue>, name: &'static str, mandatory: bool) -> Result<Option<String>> {
        match value {
            Some(v) => {
                match Self::convert_string(v) {
                    Some(s) => Ok(Some(s)),
                    None => Err(TorrentError::KeyDoesNotMapToString(name)),
                }
            },
            None => if mandatory { Err(TorrentError::MissingRequiredKey(name)) } else { Ok(None) },
        }
    }

    fn extract_uint(value: Option<&BencodeValue>, name: &'static str, mandatory: bool) -> Result<Option<u64>> {
        match value {
            Some(v) => {
                match v {
                    BencodeValue::Integer(num) => {
                        if *num < 0 {
                            Err(TorrentError::NegativeInteger(*num))
                        }
                        else {
                            Ok(Some(*num as u64))
                        }
                    },
                    _ => Err(TorrentError::KeyDoesNotMapToInteger(name))
                }
            },
            None => if mandatory { Err(TorrentError::MissingRequiredKey(name)) } else { Ok(None) },
        }
    }

    fn extract_list_of_string(value: Option<&BencodeValue>, name: &'static str, mandatory: bool) -> Result<Vec<String>> {
        let mut list: Vec<String> = Vec::new();
        match value {
            Some(v) => {
                match v {
                    BencodeValue::List(elements) => {
                        for element in elements {
                            match Self::convert_string(element) {
                                Some(s) => list.push(s),
                                None => return Err(TorrentError::KeyDoesNotMapToString(name))
                            }
                        }
                    },
                    _ => return Err(TorrentError::KeyDoesNotMapToList(name)),
                }
            },
            None => return if mandatory { Err(TorrentError::MissingRequiredKey(name)) } else { Ok(list) },
        }
        return Ok(list);
    }

    fn extract_md5sum(value: Option<&BencodeValue>) -> Result<Option<[u8; 16]>> {
        match value {
            None => Ok(None),

            Some(BencodeValue::ByteString(bytes)) => {
                let length = bytes.len();
                match bytes.as_slice().try_into() {
                    Ok(slice) => Ok(Some(slice)),
                    Err(_) => Err(TorrentError::InvalidMd5Length(length)),
                }
            }

            Some(_) => Err(TorrentError::KeyDoesNotMapToString("md5sum")),
        }
    }

    fn extract_multi_file_info(items: &BTreeMap<Vec<u8>, BencodeValue>) -> Result<MultiFileInfo> {
        let length = Self::extract_uint(items.get(LENGTH), "length", true)?.unwrap();
        let path: Vec<String> = Self::extract_list_of_string(items.get(PATH), "path", true)?;
        if path.is_empty() {
            return Err(TorrentError::KeyMapsToAnEmptyList("path"));
        }
        let md5sum = Self::extract_md5sum(items.get(MD5SUM))?;
        return Ok(MultiFileInfo { length: length, md5sum: md5sum, path: path })
    }

    fn extract_announce_list(&mut self, value: Option<&BencodeValue>) -> Result<()> {
        if let Some(v) = value {
            match v {                
                BencodeValue::List(elements) => {
                    if elements.is_empty() {
                        return Err(TorrentError::KeyMapsToAnEmptyList("announce-list"))
                    }
                    for element in elements {
                        let inner_list = match element {
                            BencodeValue::List(_) => Self::extract_list_of_string(Some(element), "announce-list", false)?,
                            BencodeValue::ByteString(bytes) => vec![Self::convert_string(element).ok_or(TorrentError::InvalidString(bytes.to_vec()))?],
                            _ => return Err(TorrentError::InvalidAnnounceListElement),
                        };
                        self.announce_list.push(inner_list);
                    }
                },
                _ => return Err(TorrentError::KeyDoesNotMapToList("announce-list"))
            }
        }
        Ok(())
    }

    fn extract_pieces(&mut self, value: Option<&BencodeValue>) -> Result<()> {
        match value {
            Some(v) => {
                match v {
                    BencodeValue::ByteString(s) => {
                        let length = s.len();
                        if length % 20 != 0 {
                            return Err(TorrentError::InvalidNumberOfPieces(length))
                        }
                        for chunk in s.chunks_exact(20) {
                            self.piece_hashes.push(chunk.try_into().unwrap());
                        }
                        Ok(())
                    },
                    _ => Err(TorrentError::KeyDoesNotMapToString("pieces"))
                }
            },
            None => Err(TorrentError::MissingRequiredKey("pieces"))
        }
    }

}