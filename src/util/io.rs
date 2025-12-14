use std::fs::{self, File, remove_file};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::metadata::file::{FileModeInfo, TorrentFile};
use crate::piece_filename;
use crate::util::md5::md5_hash;
use crate::util::to_string;

use indicatif::ProgressIterator;
use thiserror::Error;
use tracing::{info, error};

#[derive(Debug, Error)]
pub enum FileError {
    #[error("file system error: {0:?}")]
    FileSystemError(std::io::Error),
    #[error("md5 hash does not match for file {filename}: expected {expected}, received {received}")]
    Md5Mismatch{filename: String, expected: String, received: String},
    #[error("unable to write {1} bytes to {0}")]
    CopyError(String, u64),
}

#[derive(Debug, Clone)]
struct FileInfo {
    pub filepath: PathBuf,
    pub length: u64,
    pub md5sum: Option<[u8; 16]>,
}

impl FileInfo {
    fn new(filepath: PathBuf, length: u64, md5sum: Option<[u8; 16]>) -> Self {
        FileInfo { filepath, length, md5sum }
    }
}

impl FileModeInfo {
    fn files(&self) -> Box<[FileInfo]> {
        match self {
            FileModeInfo::Single {filename, length, md5sum} =>
                Box::new([FileInfo::new(PathBuf::from(filename), *length, *md5sum)]),
            FileModeInfo::Multiple {directory, files} => {
                let mut v: Vec<FileInfo> = Vec::new();
                for file in files {
                    let mut path = PathBuf::from(directory);
                    for e in &file.path {
                        path = path.join(e);
                    }
                    v.push(FileInfo::new(path, file.length, file.md5sum));
                }
                v.into_boxed_slice()
            },
        }
    }
}

pub fn reconstitute_files_from_torrent(torrent: &TorrentFile, dir: &Path) -> Result<(), FileError> {
    let files = torrent.info.files();

    let piece_paths: Vec<_> = (0..torrent.num_pieces)
        .map(|i| dir.join(piece_filename!(i)))
        .collect();

    reconstitute_files(&files, &piece_paths)?;

    for piece_path in piece_paths {
        remove_file(&piece_path).map_err(FileError::FileSystemError)?;
    }

    for file in &files {
        verify_md5(file)?
    }

    Ok(())
}

fn open_pieces_stream(piece_paths: &[PathBuf]) -> Result<Box<dyn Read>, FileError> {
    fn open_file(path: &PathBuf) -> Result<BufReader<File>, FileError> {
        Ok(BufReader::new(File::open(path).map_err(FileError::FileSystemError)?))
    }

    let mut iter = piece_paths.iter();
    let first = iter.next().unwrap();
    let mut reader: Box<dyn Read> = Box::new(open_file(first)?);

    for path in iter {
        let next = open_file(path)?;
        reader = Box::new(reader.chain(next));
    }

    Ok(reader)
}

fn reconstitute_files(infos: &[FileInfo], piece_paths: &[PathBuf]) -> Result<(), FileError> {
    info!("converting {} pieces into {} file(s)...", piece_paths.len(), infos.len());
    
    let mut reader = open_pieces_stream(piece_paths)?;

    for (i, info) in infos.iter().enumerate().progress() {
        if let Some(parent) = info.filepath.parent() {
            fs::create_dir_all(parent).map_err(FileError::FileSystemError)?
        }
        let out_file = File::create(&info.filepath).map_err(FileError::FileSystemError)?;
        let mut writer = BufWriter::new(out_file);

        let num_copied = io::copy(&mut reader.by_ref().take(info.length), &mut writer)
            .map_err(FileError::FileSystemError)?;

        if num_copied != info.length {
            return Err(FileError::CopyError(info.filepath.to_string_lossy().into(), info.length));
        }

        writer.flush().map_err(FileError::FileSystemError)?;

        info!("file {} ({}/{}) written to disk", info.filepath.to_string_lossy(), i, infos.len());
    }

    Ok(())
}

fn verify_md5(info: &FileInfo) -> Result<(), FileError> {
    if let Some(expected_hash) = info.md5sum {
        let bytes = fs::read(&info.filepath).map_err(FileError::FileSystemError)?;
        let downloaded_hash = md5_hash(&bytes);
        let path_str = info.filepath.to_string_lossy();
        return if downloaded_hash == expected_hash {
            info!("md5 of file {} matches", path_str);
            Ok(())
        } else {
            error!("md5 of file {} mis-matches: expected {}, observed {}",
                path_str, to_string(&expected_hash), to_string(&downloaded_hash));
            Err(FileError::Md5Mismatch{
                filename: info.filepath.to_string_lossy().into(),
                expected: to_string(&expected_hash),
                received: to_string(&downloaded_hash)})
        }
    }

    Ok(())
}
