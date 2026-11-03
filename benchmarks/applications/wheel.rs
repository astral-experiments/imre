use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::{env, fs};
use zip::ZipArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os().nth(1).ok_or("missing wheel")?;
    let mut wheel = ZipArchive::new(Cursor::new(fs::read(path)?))?;
    let records: Vec<_> = wheel
        .file_names()
        .filter(|path| {
            path.split_once('/')
                .is_some_and(|(dir, file)| dir.ends_with(".dist-info") && file == "RECORD")
        })
        .map(str::to_owned)
        .collect();
    let [record] = records.as_slice() else {
        return Err("expected one RECORD".into());
    };
    let mut rows = HashMap::with_capacity(wheel.len());
    for row in csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(wheel.by_name(record)?)
        .deserialize()
    {
        let (path, hash, size): (String, String, Option<u64>) = row?;
        rows.insert(path, (hash, size));
    }
    let signatures = [format!("{record}.jws"), format!("{record}.p7s")];
    let mut buffer = [0u8; 64 * 1024];
    for i in 0..wheel.len() {
        let mut member = wheel.by_index(i)?;
        if member.is_dir() {
            continue;
        }
        let name = member.name();
        let row = rows.remove(name);
        if signatures.iter().any(|signature| signature == name) {
            if row.is_some() {
                return Err("signature in RECORD".into());
            }
            continue;
        }
        let (expected, size) = row.ok_or_else(|| format!("missing RECORD entry: {name}"))?;
        if name == record {
            if !expected.is_empty() || size.is_some() {
                return Err("RECORD must have empty hash and size".into());
            }
        } else {
            let mut hash = Sha256::new();
            let mut count = 0u64;
            loop {
                let n = member.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
                count += n as u64;
            }
            if Some(count) != size
                || expected.strip_prefix("sha256=")
                    != Some(URL_SAFE_NO_PAD.encode(hash.finalize()).as_str())
            {
                return Err(format!("hash or size mismatch: {}", member.name()).into());
            }
        }
    }
    Ok(())
}
