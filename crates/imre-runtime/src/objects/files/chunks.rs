//! Read and write the actual element type of an open stream.
use super::*;

pub(in crate::objects) enum Chunk {
    Bytes(Bytes),
    Text(Str),
}

impl FileState {
    /// A bounded text read counts characters; an empty result signals EOF.
    pub(in crate::objects) fn read_chunk(&mut self, length: &Int) -> Result<Option<Chunk>> {
        if matches!(self.kind, FileKind::Text | FileKind::TextMemory) {
            let limit = usize::try_from(super::super::bytes::native_size(length)?).ok();
            let text = if self.kind == FileKind::TextMemory {
                self.text_memory()?.read(limit, false)?
            } else if let Some(limit) = limit {
                self.input()?.read_text_size(limit)?
            } else {
                self.read()?
            };
            Ok((!text.is_empty()).then_some(Chunk::Text(text)))
        } else {
            let data = self.read_size(length)?;
            Ok((!data.is_empty()).then_some(Chunk::Bytes(data)))
        }
    }

    /// A binary reader rejects writing before converting the argument. StringIO
    /// and TextIOWrapper reject a bytes argument even when already closed.
    pub(in crate::objects) fn write_chunk(&mut self, data: Chunk) -> Result<()> {
        match (data, self.kind) {
            (Chunk::Bytes(_), FileKind::TextMemory) => Err(Error::new(
                Kind::TypeError,
                "string argument expected, got 'bytes'",
            )),
            (Chunk::Bytes(_), FileKind::Text) => Err(Error::new(
                Kind::TypeError,
                "write() argument must be str, not bytes",
            )),
            (Chunk::Bytes(bytes), _) => self.write_bytes(&bytes),
            (Chunk::Text(text), FileKind::Text | FileKind::TextMemory) => self.write(&text),
            (Chunk::Text(_), _) => {
                self.check_write_mode()?;
                Err(Error::new(
                    Kind::TypeError,
                    "a bytes-like object is required, not 'str'",
                ))
            }
        }
    }
}
