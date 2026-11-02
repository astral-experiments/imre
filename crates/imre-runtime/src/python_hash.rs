//! Process-wide Python hashing for bytes and the native-width storage of Unicode strings.

use std::hash::Hasher;
use std::sync::OnceLock;

use siphasher::sip::SipHasher13;

pub(crate) const ALGORITHM: &str = "siphash13";
pub(crate) const HASH_BITS: u32 = u64::BITS;
pub(crate) const SEED_BITS: u32 = 128;
pub(crate) const CUTOFF: u32 = 0;
pub(crate) const MODULUS: i64 = (1_i64 << 61) - 1;
pub(crate) const INF: i64 = 314_159;
pub(crate) const IMAG: i64 = 1_000_003;

static HASHER: OnceLock<(SipHasher13, bool)> = OnceLock::new();

/// Capture PYTHONHASHSEED before user code can change the environment.
pub(crate) fn initialize(isolated: bool) -> &'static SipHasher13 {
    &HASHER.get_or_init(|| {
        let mut randomized = true;
        let mut key = [0; 16];
        let seed = if isolated {
            None
        } else {
            imre_ast::startup::environment_hash_seed().unwrap_or_else(|message| {
                eprintln!("{message}");
                std::process::exit(1)
            })
        };
        match seed {
            None => {
                if getrandom::fill(&mut key).is_err() {
                    eprintln!("Fatal Python error: failed to get random numbers to initialize Python\nPython runtime state: preinitialized\n");
                    std::process::exit(1);
                }
            }
            Some(seed) => {
                randomized = seed != 0;
                if randomized {
                    let mut state = seed;
                    // CPython expands an explicit seed with this 32-bit LCG.
                    for byte in &mut key {
                        state = state.wrapping_mul(214013).wrapping_add(2531011);
                        *byte = (state >> 16) as u8;
                    }
                }
            }
        }
        (SipHasher13::new_with_key(&key), randomized)
    }).0
}

/// Report whether the seed used to initialize the process hash function was nonzero or random.
pub(crate) fn randomized() -> bool {
    HASHER.get().is_none_or(|(_, randomized)| *randomized)
}

fn normalized(hash: u64) -> i64 {
    if hash == u64::MAX { -2 } else { hash as i64 }
}

pub(crate) fn bytes(data: &[u8]) -> i64 {
    if data.is_empty() {
        0
    } else {
        normalized(initialize(false).hash(data))
    }
}

pub(crate) fn text(text: &str) -> i64 {
    if text.is_ascii() {
        bytes(text.as_bytes())
    } else {
        codepoints(text.chars().map(u32::from))
    }
}

/// CPython hashes the buffer of 1-, 2- or 4-byte code units selected for the entire string.
pub(crate) fn codepoints(points: impl Iterator<Item = u32> + Clone) -> i64 {
    let Some(max) = points.clone().max() else {
        return 0;
    };
    let mut hash = *initialize(false);
    if max <= u8::MAX.into() {
        for point in points {
            hash.write(&[point as u8]);
        }
    } else if max <= u16::MAX.into() {
        for point in points {
            hash.write(&(point as u16).to_ne_bytes());
        }
    } else {
        for point in points {
            hash.write(&point.to_ne_bytes());
        }
    }
    normalized(hash.finish())
}
