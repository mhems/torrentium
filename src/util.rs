use thiserror::Error;

pub mod io;
pub mod sha1;
pub mod md5;

pub fn to_string(bytes: &[u8]) -> String {
     bytes.iter().map(|&byte| format!("{byte:02x}")).collect::<Vec<_>>().join("")
}

fn pad_bytes(bytes: &[u8], big_endian: bool) -> Vec<u8> {
    let n = bytes.len() as u64;
    let message_length: u64 = n * 8;
    let mut message: Vec<u8> = bytes.to_vec();

    message.reserve(1 + 63 + 8);
    message.push(0x80);

    while message.len() % 64 != 56 {
        message.push(0);
    }

    if big_endian {
        message.extend(message_length.to_be_bytes());
    } else {
        message.extend(message_length.to_le_bytes());
    }
    message
}

#[derive(Debug, Error)]
pub enum ConversionError {
    #[error("bytes array input must have length of 64 but has length of {0}")]
    InputMustHaveLength64(usize),
    #[error("generic const value must be at least 16 but is {0}")]
    OutputMustHaveLengthAtLeast16(usize),
    #[error("`N` ({0}) expected to be 4 * `M` ({1})")]
    NMustBeQuadrupleM(usize, usize),
}

fn to_ints<const N: usize>(bytes: &[u8], big_endian: bool) -> Result<[u32; N], ConversionError> {
    if bytes.len() != 64 {
        return Err(ConversionError::InputMustHaveLength64(bytes.len()))
    }
    if N < 16 {
        return Err(ConversionError::OutputMustHaveLengthAtLeast16(N))
    }
    let chunk: [u8; 64] = bytes.try_into().map_err(|_| ConversionError::InputMustHaveLength64(bytes.len()))?;
    let mut w: [u32; N] = [0; N];
    let f = if big_endian { u32::from_be_bytes } else { u32::from_le_bytes };

    for i in 0..16 {
        w[i] = f(chunk[i*4..i*4+4].try_into().unwrap());
    }
    Ok(w)
}

fn from_ints<const M: usize, const N: usize>(ints: [u32; M], big_endian: bool) -> Result<[u8; N], ConversionError> {
    if M * 4 != N {
        return Err(ConversionError::NMustBeQuadrupleM(N, M))
    }
    let f = if big_endian { |i: &u32| i.to_be_bytes() } else { |i: &u32| i.to_le_bytes() };
    let mut arr: [u8; N] = [0; N];
    for (i, word) in ints.iter().enumerate() {
        arr[i*4..(i+1)*4].copy_from_slice(&f(word));
    }
    Ok(arr)
}
