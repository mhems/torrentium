use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug)]
pub enum BencodeValue {
    Integer(i64),
    ByteString(Vec<u8>),
    List(Vec<BencodeValue>),
    Dictionary(BTreeMap<Vec<u8>, BencodeValue>),
}

#[derive(Debug)]
pub enum BencodeError {
    UnconsumedContents {num_remaining: usize},
    InsufficientContents,
    UnknownType {pos: usize, value: u8},
    IntegerWithLeadingZeros {pos: usize},
    EmptyInteger {pos: usize},
    IllegalInteger {pos: usize},
    UnterminatedValue {pos: usize},
    IllegalStringLength {pos: usize},
    StringMissingSeparator {pos: usize},
    IllegalDictionaryKeyType {value: String},
    DuplicateDictionaryKey {name: String},
    DictionaryKeysOutOfOrder,
}


#[derive(Debug)]
struct BencodeParser {
    contents: Vec<u8>,
    pos: usize,
    length: usize
}

fn write_bytes(bytes: &[u8], f: &mut fmt::Formatter) -> fmt::Result {
    for byte in bytes {
        write!(f, "{:02X}", byte)?;
    }
    Ok(())
}

fn write_byte_string(bytes: &[u8], f: &mut fmt::Formatter) -> fmt::Result {
    match std::str::from_utf8(bytes) {
        Ok(s) => if bytes.iter().all(|&byte| 0x20 <= byte && byte <= 0x7e) {
            write!(f, "{}", s)
        } else {
            write_bytes(bytes, f)
        }
        Err(_) => write_bytes(bytes, f),
    }
}

impl fmt::Display for BencodeValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BencodeValue::Integer(num) => write!(f, "{}", num),
            BencodeValue::ByteString(bytes) => {
                write_byte_string(bytes, f)
            }
            BencodeValue::List(elements) => {
                write!(f, "[")?;
                for element in elements {
                    write!(f, "{}\n", element)?;
                }
                write!(f, "]")?;
                Ok(())
            },
            BencodeValue::Dictionary(items ) => {
                write!(f, "{{")?;
                for (key, value) in items {
                    write_byte_string(key, f)?;
                    write!(f, " => {}\n", value)?;         
                }
                write!(f, "}}")?;
                Ok(())
            },
        }
    }
}

impl TryFrom<&[u8]> for BencodeValue {
    type Error = BencodeError;
    fn try_from(bytes: &[u8]) -> Result<Self> {
        let mut parser = BencodeParser::new(bytes);
        parser.deserialize()
    }
}

type Result<T> = std::result::Result<T, BencodeError>;

impl From<&BencodeValue> for Vec<u8> {
    fn from(value: &BencodeValue) -> Vec<u8> {
        match value {
            BencodeValue::Integer(i) => format!("i{}e", i).as_bytes().to_vec(),
            BencodeValue::ByteString(bytes) => {
                let mut v: Vec<u8> = Vec::with_capacity(10 + 1 + bytes.len());
                v.extend(format!("{}:", bytes.len()).as_bytes());
                v.extend(bytes.as_slice());
                v
            }
            BencodeValue::List(elements) => {
                let mut v: Vec<u8> = Vec::with_capacity(20 * elements.len());
                v.push(b'l');
                for element in elements {
                    v.extend(Vec::from(element).as_slice());
                }
                v.push(b'e');
                v
            },
            BencodeValue::Dictionary(items) => {
                let mut v: Vec<u8> = Vec::with_capacity(50 * items.len());
                v.push(b'd');
                for (key, value) in items {
                    v.extend(format!("{}:", key.len()).as_bytes());
                    v.extend(key.as_slice());
                    v.extend(Vec::from(value).as_slice());
                }
                v.push(b'e');
                v
            }
        }
    }
}

impl BencodeParser {

    fn new(contents: &[u8]) -> Self {
        Self {contents: contents.into(), pos: 0, length: contents.len()}
    }

    fn deserialize(&mut self) -> Result<BencodeValue> {
        let value: BencodeValue = self.parse_value()?;
        if self.pos != self.length {
            Err(BencodeError::UnconsumedContents {num_remaining: self.length - self.pos})
        } else {
            Ok(value)
        }
    }

    fn parse_value(&mut self) -> Result<BencodeValue> {
        self.ensure_available()?;
        let first: u8 = self.contents[self.pos];
        match first {
            b'i' => self.parse_integer(),
            b'l' => self.parse_list(),
            b'd' => self.parse_dictionary(),
            b'0'..=b'9' => self.parse_string(),
            _ => Err(BencodeError::UnknownType{pos: self.pos, value: first})
        }
    }

    fn parse_integer_value(&mut self, leading_zeros_allowed: bool) -> Result<i64> {
        let start = self.pos;
        loop {
            self.ensure_available()?;
            if !self.contents[self.pos].is_ascii_digit() {
                break;
            }
            self.pos += 1;
        }
        let slice = &self.contents[start..self.pos];
        if slice.is_empty() {
            return Err(BencodeError::EmptyInteger { pos: start });
        }
        if !leading_zeros_allowed && slice[0] == b'0' && self.pos > start + 1 {
            return Err(BencodeError::IntegerWithLeadingZeros { pos: start });
        }

        let s = std::str::from_utf8(slice).map_err(|_| BencodeError::IllegalInteger { pos: start })?;
        s.parse::<i64>().map_err(|_| BencodeError::IllegalInteger { pos: start })
    }

    fn parse_integer(&mut self) -> Result<BencodeValue> {
        self.pos += 1;
        let mut sign: i64 = 1;
        self.ensure_available()?;
        if self.contents[self.pos] == b'-' {
            sign = -1;
            self.pos += 1;
        }
        let value: i64 = self.parse_integer_value(false)?;
        if value == 0 && sign == -1 {
            return Err(BencodeError::IllegalInteger { pos: self.pos })
        }
        self.expect_end()?;
        self.pos += 1;
        Ok(BencodeValue::Integer(sign * value))
    }

    fn parse_string(&mut self) -> Result<BencodeValue> {
        let length: i64 = self.parse_integer_value(true)?;
        if length < 0 {
            return Err(BencodeError::IllegalStringLength { pos: self.pos })
        }
        self.ensure_available()?;
        if self.contents[self.pos] != b':' {
            return Err(BencodeError::StringMissingSeparator { pos: self.pos })
        }
        self.pos += 1;
        let mut v: Vec<u8> = Vec::with_capacity(length as usize);
        let length: u64 = length.unsigned_abs();
        for _ in 0..length {
            self.ensure_available()?;
            v.push(self.contents[self.pos]);
            self.pos += 1;
        }
        Ok(BencodeValue::ByteString(v))
    }

    fn parse_list(&mut self) -> Result<BencodeValue> {
        self.pos += 1;
        let mut values: Vec<BencodeValue> = Vec::new();
        loop {
            self.ensure_available()?;
            if self.contents[self.pos] == b'e' {
                break
            }
            values.push(self.parse_value()?);
        }
        self.pos += 1;
        Ok(BencodeValue::List(values))
    }

    fn parse_dictionary(&mut self) -> Result<BencodeValue> {
        self.pos += 1;
        let mut map: BTreeMap<Vec<u8>, BencodeValue> = BTreeMap::new();
        loop {
            self.ensure_available()?;
            if self.contents[self.pos] == b'e' {
                break
            }
            let key: BencodeValue = self.parse_string()?;
            let key_bytes = match &key {
                BencodeValue::ByteString (bytes ) => bytes,
                _ => return Err(BencodeError::IllegalDictionaryKeyType { value: key.to_string() })
            };

            if let Some(pair) = map.last_key_value() {
                if key_bytes < pair.0 {
                    return Err(BencodeError::DictionaryKeysOutOfOrder);
                } else if key_bytes == pair.0 {
                    return Err(BencodeError::DuplicateDictionaryKey { name: key.to_string() })
                }
            }
            
            let value: BencodeValue = self.parse_value()?;
            map.insert(key_bytes.to_vec(), value);
        }
        self.pos += 1;
        Ok(BencodeValue::Dictionary(map))
    }

    fn ensure_available<>(&self) -> Result<()> {
        if self.pos >= self.length {
            Err(BencodeError::InsufficientContents)
        } else {
            Ok(())
        }
    }

    fn expect_end<>(&self) -> Result<()> {
        self.ensure_available()?;
        if self.contents[self.pos] != b'e' {
            Err(BencodeError::UnterminatedValue { pos: self.pos })
        } else {
            Ok(())
        }
    }
}
