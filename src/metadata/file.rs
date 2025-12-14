use std::collections::BTreeMap;
use std::net::SocketAddrV4;
use std::path::Path;
use std::{fmt, fs};

use url::Url;
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::PEER_ID;
use crate::peer::download;
use crate::util::sha1::sha1_hash;
use crate::util::io::reconstitute_files_from_torrent;
use crate::metadata::tracker::{TrackerError, TrackerResponse, retrieve_peers};
use crate::metadata::bencode::{BencodeError, BencodeValue};

#[derive(Debug, Clone)]
pub struct TorrentFile {
    announce: String,
    announce_list: Vec<Vec<String>>,
    creation_date: Option<u64>,
    comment: Option<String>,
    created_by: Option<String>,
    encoding: Option<String>,
    private: bool,

    pub info: FileModeInfo,
    pub total_num_bytes: u64,
    pub num_bytes_per_piece: u64,
    pub num_pieces: usize,
    pub piece_hashes: Vec<[u8; 20]>,
    pub hash: [u8; 20],

    pub filename: String,
}

#[derive(Debug, Clone)]
pub enum FileModeInfo {
    Single {filename: String, length: u64, md5sum: Option<[u8; 16]>},
    Multiple {directory: String, files: Vec<MultiFileInfo>},
}

#[derive(Debug, Clone)]
pub struct MultiFileInfo {
    pub length: u64,
    pub md5sum: Option<[u8; 16]>,
    pub path: Vec<String>,
}

#[derive(Debug, Error)]
pub enum TorrentFileError {
    #[error("invalid file path")]
    InvalidFilePath,
    #[error("unable to open file {0}: {1:?}")]
    FileReadError(String, std::io::Error),
    #[error("file {0} is not b-encoded: {1:?}")]
    BencodeError(String, BencodeError),
    #[error("torrent file is expected to be a dictionary")]
    FileIsNotDictionary,
    #[error("key `{0}` expected to map to a string")]
    KeyDoesNotMapToString(&'static str),
    #[error("missing required key `{0}`")]
    MissingRequiredKey(&'static str),
    #[error("key `{0}` expected to map to an integer")]
    KeyDoesNotMapToInteger(&'static str),
    #[error("integer `{0}` cannot be negative")]
    NegativeInteger(i64),
    #[error("key `{0}` expected to map to a dictionary")]
    KeyDoesNotMapToDictionary(&'static str),
    #[error("key `{0}` expected to map to a list")]
    KeyDoesNotMapToList(&'static str),
    #[error("md5sum length expected to be 16 but is {0}")]
    InvalidMd5Length(usize),
    #[error("the `private` key expected to map to an integer of value 0 or 1 but is {0}")]
    InvalidPrivateValue(u64),
    #[error("key `{0}` expected to map to a list of strings")]
    KeyDoesNotMapToListOfStrings(&'static str),
    #[error("key `{0}` expected to map to a non-empty list")]
    KeyMapsToAnEmptyList(&'static str),
    #[error("`pieces` byte string expected to a length which is a multiple of 20 but is {0}")]
    InvalidNumberOfPieces(usize),
    #[error("`announce-list` expected to map to either a byte string or list of strings")]
    InvalidAnnounceListElement,
    #[error("byte string `{0:?}` is not representible in ASCII")]
    InvalidString(Vec<u8>),
    #[error("unable to parse `announce` URL '{0}'")]
    InvalidAnnounceUrl(String),
    #[error("file length totals {0} do not align with piece totals {1}")]
    LengthMismatch(u64, u64),
}

type Result<T> = std::result::Result<T, TorrentFileError>;

impl fmt::Display for FileModeInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FileModeInfo::Single{filename, length, ..} => {
                write!(f, "{} ({})", filename, to_human_bytes(*length))?;
                Ok(())
            },
            FileModeInfo::Multiple { directory, files } => {
                let file_list = files
                    .iter()
                    .map(|i| format!("{} ({})", i.path.join("/"), to_human_bytes(i.length)))
                    .collect::<Vec<_>>()
                    .join(", ");

                write!(f, "[{file_list}] -> {directory}/")
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
        if let Some(seconds) = &self.creation_date {
            let created_str = OffsetDateTime::from_unix_timestamp(*seconds as i64)
                .ok()
                .and_then(|dt| dt.format(&Rfc3339).ok())
                .unwrap_or_else(|| format!("{seconds} seconds since Epoch"));
            writeln!(f, "created: {created_str}")?;
        }
        if let Some(text) = &self.comment {
            writeln!(f, "comment: {text}")?;
        }
        if let Some(author) = &self.created_by {
            writeln!(f, "created by: {author}")?;
        }
        if let Some(e) = &self.encoding {
            writeln!(f, "encoding: {e}")?;
        }
        writeln!(f, "private?: {}", self.private)?;
        writeln!(f, "num hashes: {}", self.piece_hashes.len())?;
        writeln!(f, "total size: {} ({} pieces of {} each)",
            to_human_bytes(self.total_num_bytes),
            self.num_pieces,
            to_human_bytes(self.num_bytes_per_piece))?;
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

impl TorrentFile {
    pub fn new<P: AsRef<Path>>(filepath: P) -> Result<Self> {
        let filename = filepath.as_ref()
            .file_name()
            .and_then(|name| name.to_string_lossy().into())
            .ok_or(TorrentFileError::InvalidFilePath)?.to_string();

        match fs::read(filepath) {
            Ok(contents) => {
                match BencodeValue::try_from(contents.as_slice()) {
                    Ok(bencode_value) => {
                        match bencode_value {
                            BencodeValue::Dictionary(items) => {
                                TorrentFile::extract(&filename, &items)
                            },
                            _ => Err(TorrentFileError::FileIsNotDictionary)
                        }
                    },
                    Err(e) => Err(TorrentFileError::BencodeError(filename, e)),
                }
            }
            Err(e) => Err(TorrentFileError::FileReadError(filename, e)),
        }
    }

    pub async fn retrieve_peers(&self) -> std::result::Result<TrackerResponse, TrackerError> {
        let url = self.get_announce_url(self.total_num_bytes, PEER_ID, 12345);
        retrieve_peers(url).await
    }

    pub async fn download(&self, peers: &[SocketAddrV4]) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::TempDir::new().expect("should be able to construct temporary directory");
        let dir_path = dir.path();

        download(peers, self, dir_path).await?;

        reconstitute_files_from_torrent(self, dir_path).map_err(|e| e.into())
    }

    fn extract(filename: &str, items: &BTreeMap<Vec<u8>, BencodeValue>) -> Result<Self> {
        let announce = Self::extract_string(items.get(ANNOUNCE), "announce", true)?.unwrap();
        let _ = Url::parse(&announce).map_err(|_| TorrentFileError::InvalidAnnounceUrl(announce.to_string()))?;
        let announce_list = Self::extract_announce_list(items.get(ANNOUNCE_LIST))?;
        let creation_date = Self::extract_uint(items.get(CREATION_DATE), "creation date", false)?;
        let comment = Self::extract_string(items.get(COMMENT), "comment", false)?;
        let created_by = Self::extract_string(items.get(CREATED_BY), "created by", false)?;
        let encoding = Self::extract_string(items.get(ENCODING), "encoding", false)?;
        let info_items = match items.get(INFO) {
            Some(bencoded_value) => {
                match bencoded_value {
                    BencodeValue::Dictionary(items) => items,
                    _ => return Err(TorrentFileError::KeyDoesNotMapToDictionary("info")),
                }
            },
            None => return Err(TorrentFileError::MissingRequiredKey("info")),
        };
        let num_bytes_per_piece = Self::extract_uint(info_items.get(PIECE_LENGTH), "piece length", true)?.unwrap();
        let private_int = Self::extract_uint(info_items.get(PRIVATE), "private", false)?;
        let private = if let Some(v) = private_int {
            if v <= 1 {
                v == 0
            } else {
                return Err(TorrentFileError::InvalidPrivateValue(v));
            }
        } else {
            false
        };
        let piece_hashes = Self::extract_pieces(info_items.get(PIECES))?;
        let num_pieces = piece_hashes.len();
        let hash = sha1_hash(Vec::from(items.get(INFO).unwrap()).as_slice());
        let name = Self::extract_string(info_items.get(NAME), "name", true)?.unwrap();
        let (info, total_num_bytes) = if info_items.contains_key(FILES) {
            let mut files = Vec::new();
            let mut length: u64 = 0;
            match info_items.get(FILES) {
                Some(v) => {
                    match v {
                        BencodeValue::List(elements) => {
                            for element in elements {
                                match element {
                                    BencodeValue::Dictionary(items) => {
                                        let e = Self::extract_multi_file_info(items)?;
                                        length += e.length;
                                        files.push(e);
                                    },
                                    _ => return Err(TorrentFileError::KeyDoesNotMapToListOfStrings("files"))
                                }
                            }
                        },
                        _ => return Err(TorrentFileError::KeyDoesNotMapToListOfStrings("files"))
                    }
                },
                None => return Err(TorrentFileError::MissingRequiredKey("files"))
            }
            if files.is_empty() {
                return Err(TorrentFileError::KeyMapsToAnEmptyList("files"))
            }
            (FileModeInfo::Multiple { directory: name, files }, length)
        } else {
            let length = Self::extract_uint(info_items.get(LENGTH), "length", true)?.unwrap();
            let md5sum = Self::extract_md5sum(info_items.get(MD5SUM))?;
            (FileModeInfo::Single { filename: name, length, md5sum }, length)
        };

        let np = num_pieces as u64;
        let upper_bound = num_bytes_per_piece * np;
        if num_bytes_per_piece * (np - 1) >= total_num_bytes ||
           total_num_bytes > upper_bound {
            return Err(TorrentFileError::LengthMismatch(total_num_bytes, upper_bound));
        }

        Ok(TorrentFile {
            announce,
            announce_list,
            creation_date,
            comment,
            created_by,
            encoding,
            info,
            total_num_bytes,
            num_bytes_per_piece,
            num_pieces,
            piece_hashes,
            hash,
            private,
            filename: filename.to_owned()
        })
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
                    None => Err(TorrentFileError::KeyDoesNotMapToString(name)),
                }
            },
            None => if mandatory { Err(TorrentFileError::MissingRequiredKey(name)) } else { Ok(None) },
        }
    }

    pub(crate) fn extract_uint(value: Option<&BencodeValue>, name: &'static str, mandatory: bool) -> Result<Option<u64>> {
        match value {
            Some(v) => {
                match v {
                    BencodeValue::Integer(num) => {
                        if *num < 0 {
                            Err(TorrentFileError::NegativeInteger(*num))
                        }
                        else {
                            Ok(Some(num.unsigned_abs()))
                        }
                    },
                    _ => Err(TorrentFileError::KeyDoesNotMapToInteger(name))
                }
            },
            None => if mandatory { Err(TorrentFileError::MissingRequiredKey(name)) } else { Ok(None) },
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
                                None => return Err(TorrentFileError::KeyDoesNotMapToString(name))
                            }
                        }
                    },
                    _ => return Err(TorrentFileError::KeyDoesNotMapToList(name)),
                }
            },
            None => return if mandatory { Err(TorrentFileError::MissingRequiredKey(name)) } else { Ok(list) },
        }
        Ok(list)
    }

    fn extract_md5sum(value: Option<&BencodeValue>) -> Result<Option<[u8; 16]>> {
        match value {
            None => Ok(None),

            Some(BencodeValue::ByteString(bytes)) => {
                let length = bytes.len();
                match bytes.as_slice().try_into() {
                    Ok(slice) => Ok(Some(slice)),
                    Err(_) => Err(TorrentFileError::InvalidMd5Length(length)),
                }
            }

            Some(_) => Err(TorrentFileError::KeyDoesNotMapToString("md5sum")),
        }
    }

    fn extract_multi_file_info(items: &BTreeMap<Vec<u8>, BencodeValue>) -> Result<MultiFileInfo> {
        let length = Self::extract_uint(items.get(LENGTH), "length", true)?.unwrap();
        let path: Vec<String> = Self::extract_list_of_string(items.get(PATH), "path", true)?;
        if path.is_empty() {
            return Err(TorrentFileError::KeyMapsToAnEmptyList("path"));
        }
        let md5sum = Self::extract_md5sum(items.get(MD5SUM))?;
        Ok(MultiFileInfo { length, md5sum, path })
    }

    fn extract_announce_list(value: Option<&BencodeValue>) -> Result<Vec<Vec<String>>> {
        let mut announce_list = Vec::new();
        if let Some(v) = value {
            match v {                
                BencodeValue::List(elements) => {
                    if elements.is_empty() {
                        return Err(TorrentFileError::KeyMapsToAnEmptyList("announce-list"))
                    }
                    for element in elements {
                        let inner_list = match element {
                            BencodeValue::List(_) => Self::extract_list_of_string(Some(element), "announce-list", false)?,
                            BencodeValue::ByteString(bytes) => vec![Self::convert_string(element).ok_or(TorrentFileError::InvalidString(bytes.clone()))?],
                            _ => return Err(TorrentFileError::InvalidAnnounceListElement),
                        };
                        announce_list.push(inner_list);
                    }
                },
                _ => return Err(TorrentFileError::KeyDoesNotMapToList("announce-list"))
            }
        }
        Ok(announce_list)
    }

    fn extract_pieces(value: Option<&BencodeValue>) -> Result<Vec<[u8; 20]>> {
        match value {
            Some(v) => {
                match v {
                    BencodeValue::ByteString(s) => {
                        let length = s.len();
                        if length % 20 != 0 {
                            return Err(TorrentFileError::InvalidNumberOfPieces(length))
                        }
                        let mut hashes: Vec<[u8; 20]> = Vec::with_capacity(length/20);
                        for chunk in s.chunks_exact(20) {
                            hashes.push(chunk.try_into().unwrap());
                        }
                        Ok(hashes)
                    },
                    _ => Err(TorrentFileError::KeyDoesNotMapToString("pieces"))
                }
            },
            None => Err(TorrentFileError::MissingRequiredKey("pieces"))
        }
    }

    fn get_announce_url(&self, length: u64, peer_id: &[u8;20], port: u16) -> Url {
        let mut url = Url::parse(&self.announce).expect("announce URL verified on parse");

        let encoded_hash = percent_encode(self.hash.as_slice(), NON_ALPHANUMERIC).to_string();
        let encoded_id = percent_encode(peer_id, NON_ALPHANUMERIC).to_string();

        url.query_pairs_mut()
            .append_pair("port", &port.to_string())
            .append_pair("uploaded", "0")
            .append_pair("downloaded", "0")
            .append_pair("compact", "1")
            .append_pair("left", &length.to_string());

        let new_url_str = format!("{url}&info_hash={encoded_hash}&peer_id={encoded_id}");
        Url::parse(&new_url_str).expect("internally formed URL expected to be valid")
    }
}


fn to_human_bytes(num_bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut unit = 0;
    let mut num: f64 = num_bytes as f64;
    while num >= 1024.0 && unit < UNITS.len() - 1 {
        num /= 1024.0;
        unit += 1;
    }

    format!("{:.1} {}", num, UNITS[unit])
}