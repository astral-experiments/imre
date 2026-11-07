//! Single-byte codec tables shared by source decoding and the text codec registry.

pub(super) const NAMES: &str = include_str!("linecache/codec_names.txt");
const TABLES: &[u8] = include_bytes!("linecache/charmaps.bin");

#[derive(Clone, Copy)]
pub(super) struct Charmap(&'static [u8]);

impl Charmap {
    /// Each pinned table contains 256 little-endian Unicode scalars.
    pub(super) const fn at(index: usize) -> Self {
        let (_, tail) = TABLES.split_at(index * 512);
        Self(tail.split_at(512).0)
    }

    /// U+FFFE is the charmap sentinel for an undefined input byte.
    pub(super) fn decode(self, byte: u8) -> Option<char> {
        let at = usize::from(byte) * 2;
        let scalar = u16::from_le_bytes([self.0[at], self.0[at + 1]]);
        (scalar != 0xfffe).then(|| char::from_u32(u32::from(scalar)).unwrap())
    }
}
