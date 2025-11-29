
use std::collections::HashMap;
use std::panic;
use std::fmt;

type ByteArray = Box<[u8]>;

#[derive(Debug)]
pub enum BencodeValue {
    Integer(i64),
    String(std::string::String),
    List(Vec<BencodeValue>),
    Dictionary(HashMap<std::string::String, Box<BencodeValue>>),
}

#[derive(Debug)]
pub struct BencodeParser {
    contents: ByteArray,
    pos: usize,
    length: usize
}

impl fmt::Display for BencodeValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        return match self {
            BencodeValue::Integer(num) => write!(f, "{}", num),
            BencodeValue::String(text) => write!(f, "{}", text),
            BencodeValue::List(elements) => {
                let s: Vec<std::string::String> = elements.iter().map(|e| e.to_string()).collect();
                return write!(f, "[{}]", s.join("\n"));
            },
            BencodeValue::Dictionary(items ) => {
                let s: Vec<std::string::String> = items.iter().map(|pair| pair.0.to_owned() + " => " + &pair.1.to_string()).collect();
                return write!(f, "{{{}}}", s.join("\n"));
            },
        };
    }
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
        return match first {
            b'i' => self.parse_integer(),
            b'l' => self.parse_list(),
            b'd' => self.parse_dictionary(),
            b'1'..=b'9' => self.parse_string(),
            _ => panic!("unknown type specificer {}", first),
        };
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

    fn parse_integer_value(&mut self) -> i64 {
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
        let mut place: i64 = 1;
        let mut value: i64 = 0;
        for digit in digits.iter().rev() {
            value += place * i64::from(*digit);
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
        let value: i64 = i64::from(self.parse_integer_value());
        if value == 0 && sign == -1 {
            panic!("negative zero is not allowed")
        }
        if self.contents[self.pos] != b'e' {
            panic!("integers must end with 'e'")
        }
        self.pos += 1;
        return BencodeValue::Integer(sign * value);
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
        return BencodeValue::List(values);
    }

    fn parse_dictionary(&mut self) -> BencodeValue {
        self.verify_pos(None);
        if self.contents[self.pos] != b'd' {
            panic!("dictionaries must start with 'd'");
        }
        self.pos += 1;
        self.verify_pos(None);
        let mut map: HashMap<std::string::String, Box<BencodeValue>> = HashMap::new();
        let mut last_added: Option<std::string::String> = None;
        while self.contents[self.pos] != b'e' {
            self.verify_pos(None);
            let key: BencodeValue = self.parse_string();
            let b = match key {
                BencodeValue::String (text ) => text,
                _ => panic!("dictionary keys must be strings"),
            };
            if map.contains_key(&b) {
                panic!("dictionary keys must be unique");
            }
            match last_added {
                Some(arr) => if b < arr {
                    panic!("dictionary keys must appear in order");
                },
                None => (),
            };
            last_added = Some(b.clone());
            let value: Box<BencodeValue> = Box::new(self.parse_helper());
            map.insert(b, value);

        }
        self.pos += 1;
        return BencodeValue::Dictionary(map);
    }

    fn parse_string(&mut self) -> BencodeValue {
        self.verify_pos(None);
        let return_value: i64 = self.parse_integer_value();
        if return_value < 0 {
            panic!("strings cannot have negative lengths");
        }
        self.verify_pos(None);
        if self.contents[self.pos] != b':' {
            panic!("':' expected to separate string length from string contents")
        }
        self.pos += 1;
        let mut s: String = String::new();
        let length: u64 = return_value.unsigned_abs();
        for _ in 0..length {
            self.verify_pos(None);
            s.push(char::from(self.contents[self.pos]));
            self.pos += 1;
        }
        return BencodeValue::String(s);
    }

}

