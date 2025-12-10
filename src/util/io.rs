use std::fs::{File, remove_file};
use std::io::{self, BufReader, BufWriter, copy, Write};
use std::path::Path;

pub fn concatenate_pieces<P: AsRef<Path>>(input_files: &[P], output_file: &P) -> io::Result<()> {
    let out = File::create(output_file)?;
    let mut writer = BufWriter::new(out);

    for file_path in input_files {
        let input = File::open(file_path)?;
        let mut reader = BufReader::new(input);

        copy(&mut reader, &mut writer)?;
        
        remove_file(file_path)?;
    }

    writer.flush()?;
    Ok(())
}
