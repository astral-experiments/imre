//! Single-byte codec tables shared by source decoding and the text codec registry.

pub(super) const NAMES: &str = include_str!("linecache/codec_names.txt");
const TABLES: &[u8] = include_bytes!("linecache/charmaps.bin");

#[derive(Clone, Copy)]
pub(super) struct Charmap(&'static [u8]);

impl Charmap {
    /// Each pinned table contains 256 little-endian Unicode scalars.
    pub(super) fn at(index: usize) -> Self {
        Self(&TABLES[index * 512..(index + 1) * 512])
    }

    /// Find the table index maintained with CPython's codec aliases.
    pub(super) fn named(name: &str) -> Option<Self> {
        NAMES
            .lines()
            .filter_map(|line| line.split_once(' '))
            .find(|(alias, _)| *alias == name)?
            .1
            .parse()
            .ok()
            .map(Self::at)
    }

    /// U+FFFE is the charmap sentinel for an undefined input byte.
    pub(super) fn decode(self, byte: u8) -> Option<char> {
        let at = usize::from(byte) * 2;
        let scalar = u16::from_le_bytes([self.0[at], self.0[at + 1]]);
        (scalar != 0xfffe).then(|| char::from_u32(u32::from(scalar)).unwrap())
    }
}
