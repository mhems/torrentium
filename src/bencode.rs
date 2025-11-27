
use std::collections::HashMap;
use std::panic;

type ByteArray = Box<[u8]>;

#[derive(Debug)]
pub enum BencodeValue {
    Integer {value: i64},
    String {value: ByteArray},
    List {elements: Vec<BencodeValue>},
    Dictionary {items: HashMap<ByteArray, Box<BencodeValue>>},
}

#[derive(Debug)]
pub struct BencodeParser {
    contents: ByteArray,
    pos: usize,
    length: usize
}

impl BencodeParser {

    pub fn new(contents: &[u8]) -> Self {
        Self {contents: contents.into(), pos: 0, length: contents.len()}
    }

    pub fn parse(&mut self) -> BencodeValue {
        let value: BencodeValue = self.parse_helper();
        if self.pos != self.length {
            panic!("unconsumed contents (pos={} vs len={})", self.pos, self.length)
        }
        return value;
    }

    fn parse_helper(&mut self) -> BencodeValue {
        if self.pos >= self.length {
            panic!("no further values");
        }
        let first: u8 = self.contents[self.pos];
        let x: BencodeValue = match first {
            b'i' => self.parse_integer(),
            b'l' => self.parse_list(),
            b'd' => self.parse_dictionary(),
            b'1'..=b'9' => self.parse_string(),
            _ => panic!("unknown type specificer {}", first),
        };
        return x;
    }

    fn verify_pos(&self, addend: Option<usize>) {
        let n: usize = match addend {
            Some(value) => value,
            None => 0, 
        };
        if self.pos + n >= self.length {
            panic!("expected further content")
        }
    }

    fn parse_integer_value(&mut self) -> u64 {
        self.verify_pos(None);
        if self.contents[self.pos] == b'0' {
            self.verify_pos(Some(1));
            self.pos += 1;
            match self.contents[self.pos] {
                b'0'..=b'9' => panic!("cannot have leading zeros"),
                _ => return 0,
            }
        }
        let mut digits: Vec<u8> = Vec::new();
        while self.pos < self.length {
            let cur: u8 = self.contents[self.pos];
            match cur {
                b'0'..=b'9' => digits.push(cur - b'0'),
                _ => break,
            }
            self.pos += 1;
        }
        let mut place: u64 = 1;
        let mut value: u64 = 0;
        for digit in digits.iter().rev() {
            value += place * (*digit as u64);
            place *= 10;
        }
        return value;

    }

    fn parse_integer(&mut self) -> BencodeValue {
        self.verify_pos(None);
        let first: u8 = self.contents[self.pos];
        if first != b'i' {
            panic!("integers must start with 'i'");
        }
        self.pos += 1;
        self.verify_pos(None);
        let mut sign: i64 = 1;
        if self.contents[self.pos] == b'-' {
            sign = -1;
            self.pos += 1;
        }
        let value: i64 = self.parse_integer_value() as i64;
        if self.contents[self.pos] != b'e' {
            panic!("integers must end with 'e'")
        }
        self.pos += 1;
        return BencodeValue::Integer{value: sign * value};
    }

    fn parse_list(&mut self) -> BencodeValue {
        self.verify_pos(None);
        if self.contents[self.pos] != b'l' {
            panic!("lists must start with 'l'");
        }
        self.pos += 1;
        self.verify_pos(None);
        let mut values: Vec<BencodeValue> = Vec::new();
        while self.contents[self.pos] != b'e' {
            values.push(self.parse_helper());
        }
        self.pos += 1;
        return BencodeValue::List { elements: values };

    }

    fn parse_dictionary(&mut self) -> BencodeValue {
        self.verify_pos(None);
        if self.contents[self.pos] != b'd' {
            panic!("dictionaries must start with 'd'");
        }
        self.pos += 1;
        self.verify_pos(None);
        let mut map: HashMap<ByteArray, Box<BencodeValue>> = HashMap::new();
        while self.contents[self.pos] != b'e' {
            self.verify_pos(None);
            let key: BencodeValue = self.parse_string();
            let b: ByteArray = match key {
                BencodeValue::String { value } => value,
                _ => panic!("dictionary keys must be strings"),
            };
            let value: Box<BencodeValue> = Box::new(self.parse_helper());
            map.insert(b, value);
        }
        self.pos += 1;
        return BencodeValue::Dictionary { items: map };
    }

    fn parse_string(&mut self) -> BencodeValue {
        self.verify_pos(None);
        let length: usize = self.parse_integer_value() as usize;
        self.verify_pos(None);
        if self.contents[self.pos] != b':' {
            println!("pos {} = {}", self.pos, self.contents[self.pos] as char);
            panic!("':' expected to separate string length from string contents")
        }
        self.pos += 1;
        let mut s: String = String::new();
        for _ in 0..length {
            self.verify_pos(None);
            s.push(self.contents[self.pos] as char);
            self.pos += 1;
        }
        return BencodeValue::String { value: Box::from(s.as_bytes()) };
    }
}

