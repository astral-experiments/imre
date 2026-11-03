use memchr::{memchr_iter, memchr2, memmem::Finder};
use std::fs;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    let needle = Finder::new("TODO");
    let manifest = fs::read_to_string(std::env::args_os().nth(1).expect("missing manifest"))?;
    for path in manifest.lines() {
        let source = fs::read_to_string(path)?;
        let mut text = source.as_bytes();
        let mut line = 1;
        while let Some(hit) = needle.find(text) {
            let prefix = &text[..hit];
            line += memchr_iter(b'\n', prefix).count();
            // Python text mode also treats a lone carriage return as a newline.
            line += memchr_iter(b'\r', prefix)
                .filter(|&pos| prefix.get(pos + 1) != Some(&b'\n'))
                .count();
            writeln!(out, "{path}:{line}")?;
            let Some(end) = memchr2(b'\r', b'\n', &text[hit..]) else {
                break;
            };
            let end = hit + end;
            let crlf = text[end] == b'\r' && text.get(end + 1) == Some(&b'\n');
            text = &text[end + 1 + usize::from(crlf)..];
            line += 1;
        }
    }
    out.flush()
}
