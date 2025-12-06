pub mod metadata;

mod sha1;

pub mod peer;

pub type Bitfield = Vec<u8>;

pub fn has_piece(field: &Bitfield, index: usize) -> bool {
    let element_index = index / 8;
    let element_offset = index % 8;
    let mask = 1 << (7 - element_offset);
    field[element_index] & mask == mask
}

pub fn mark_piece(field: &mut Bitfield, index: usize) {
    let element_index = index / 8;
    let element_offset = index % 8;
    let mask = 1 << (7 - element_offset);
    field[element_index] |= mask;
}
