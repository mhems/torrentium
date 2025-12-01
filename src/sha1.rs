
const H0: u32 = 0x67452301;
const H1: u32 = 0xEFCDAB89;
const H2: u32 = 0x98BADCFE;
const H3: u32 = 0x10325476;
const H4: u32 = 0xC3D2E1F0;

pub fn sha1_hash(bytes: &[u8]) -> [u8; 20] {
    let n = bytes.len() as u64;
    let message_length: u64 = n * 8;
    let mut message: Vec<u8> = bytes.to_vec();
    message.reserve(1 + 63 + 8);
    message.push(0x80);
    while (message.len() * 8) % 512 != 448 {
        message.push(0);
    };
    message.extend(message_length.to_be_bytes());

    let mut h0 = H0;
    let mut h1 = H1;
    let mut h2 = H2;
    let mut h3 = H3;
    let mut h4 = H4;

    for chunk in message.chunks_exact(64) {
        let mut w: [u32; 80] = [0; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i*4..i*4+4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..=79 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a.rotate_left(5)
                             .wrapping_add(f)
                             .wrapping_add(e)
                             .wrapping_add(k)
                             .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    };

    let mut hash: [u8; 20] = [0; 20];
    for (i, word) in [h0, h1, h2, h3, h4].iter().enumerate() {
        hash[i*4..(i+1)*4].copy_from_slice(&word.to_be_bytes());
    }
    hash
}