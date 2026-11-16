//! Hash objects share mutable state. Digest reads finalize an independent snapshot.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ptr;

use md5::Md5;
use num_traits::{Signed, ToPrimitive};
use sha1::Sha1;
use sha2::digest::common::BlockSizeUser;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use sha3::digest::ExtendableOutput;
use sha3::{Sha3_224, Sha3_256, Sha3_384, Sha3_512, Shake128, Shake256};

use super::exceptions::{Error, Kind, Result, finish};
use super::{Bytes, Object, Storage, StorageCell, Str, object};
use crate::dynamic::DigestState;
use crate::{BytesOwner, Int, ObjectOwner};

#[derive(Clone)]
pub(crate) struct State {
    algorithm: Algorithm,
    class: &'static str,
}

#[derive(Clone)]
enum Algorithm {
    Blake2b(blake2b_simd::State, usize),
    Blake2s(blake2s_simd::State, usize),
    Md5(Md5),
    Sha1(Sha1),
    Sha224(Sha224),
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
    Sha3_224(Sha3_224),
    Sha3_256(Sha3_256),
    Sha3_384(Sha3_384),
    Sha3_512(Sha3_512),
    Shake128(Shake128),
    Shake256(Shake256),
}

pub(crate) struct Provider {
    pub(crate) name: &'static str,
    aliases: &'static [&'static str],
    initialize: fn(&[u8]) -> Algorithm,
}

impl Provider {
    pub(crate) fn create(&self, class: Option<&'static str>, data: &[u8]) -> State {
        State::new((self.initialize)(data), class)
    }
}

const PROVIDERS: &[Provider] = &[
    Provider {
        name: "blake2b",
        aliases: &[],
        initialize: |data| {
            let mut state = blake2b_simd::State::new();
            state.update(data);
            Algorithm::Blake2b(state, 64)
        },
    },
    Provider {
        name: "blake2s",
        aliases: &[],
        initialize: |data| {
            let mut state = blake2s_simd::State::new();
            state.update(data);
            Algorithm::Blake2s(state, 32)
        },
    },
    Provider {
        name: "md5",
        aliases: &["ssl3-md5", "1.2.840.113549.2.5"],
        initialize: |data| Algorithm::Md5(Md5::new_with_prefix(data)),
    },
    Provider {
        name: "sha1",
        aliases: &["sha-1", "ssl3-sha1", "1.3.14.3.2.26"],
        initialize: |data| Algorithm::Sha1(Sha1::new_with_prefix(data)),
    },
    Provider {
        name: "sha224",
        aliases: &["sha-224", "sha2-224", "2.16.840.1.101.3.4.2.4"],
        initialize: |data| Algorithm::Sha224(Sha224::new_with_prefix(data)),
    },
    Provider {
        name: "sha256",
        aliases: &["sha-256", "sha2-256", "2.16.840.1.101.3.4.2.1"],
        initialize: |data| Algorithm::Sha256(Sha256::new_with_prefix(data)),
    },
    Provider {
        name: "sha384",
        aliases: &["sha-384", "sha2-384", "2.16.840.1.101.3.4.2.2"],
        initialize: |data| Algorithm::Sha384(Sha384::new_with_prefix(data)),
    },
    Provider {
        name: "sha512",
        aliases: &["sha-512", "sha2-512", "2.16.840.1.101.3.4.2.3"],
        initialize: |data| Algorithm::Sha512(Sha512::new_with_prefix(data)),
    },
    Provider {
        name: "sha3_224",
        aliases: &["sha3-224", "2.16.840.1.101.3.4.2.7"],
        initialize: |data| Algorithm::Sha3_224(Sha3_224::new_with_prefix(data)),
    },
    Provider {
        name: "sha3_256",
        aliases: &["sha3-256", "2.16.840.1.101.3.4.2.8"],
        initialize: |data| Algorithm::Sha3_256(Sha3_256::new_with_prefix(data)),
    },
    Provider {
        name: "sha3_384",
        aliases: &["sha3-384", "2.16.840.1.101.3.4.2.9"],
        initialize: |data| Algorithm::Sha3_384(Sha3_384::new_with_prefix(data)),
    },
    Provider {
        name: "sha3_512",
        aliases: &["sha3-512", "2.16.840.1.101.3.4.2.10"],
        initialize: |data| Algorithm::Sha3_512(Sha3_512::new_with_prefix(data)),
    },
    Provider {
        name: "shake_128",
        aliases: &["shake128", "shake-128", "2.16.840.1.101.3.4.2.11"],
        initialize: |data| {
            Algorithm::Shake128(sha3::digest::Update::chain(Shake128::default(), data))
        },
    },
    Provider {
        name: "shake_256",
        aliases: &["shake256", "shake-256", "2.16.840.1.101.3.4.2.12"],
        initialize: |data| {
            Algorithm::Shake256(sha3::digest::Update::chain(Shake256::default(), data))
        },
    },
];

impl State {
    fn new(algorithm: Algorithm, class: Option<&'static str>) -> Self {
        // Compatibility aliases select the generic native constructor even when
        // their canonical spelling is provided by a dedicated Python class.
        let class = class.unwrap_or(match algorithm {
            Algorithm::Shake128(_) | Algorithm::Shake256(_) => Self::XOF_CLASS,
            _ => Self::CLASS,
        });
        Self { algorithm, class }
    }

    pub(crate) fn blake2b(state: blake2b_simd::State, size: usize) -> Self {
        Self::new(Algorithm::Blake2b(state, size), Some("_blake2.blake2b"))
    }

    pub(crate) fn blake2s(state: blake2s_simd::State, size: usize) -> Self {
        Self::new(Algorithm::Blake2s(state, size), Some("_blake2.blake2s"))
    }

    pub(crate) fn is_xof(&self) -> bool {
        matches!(
            self.algorithm,
            Algorithm::Shake128(_) | Algorithm::Shake256(_)
        )
    }

    pub(crate) fn algorithms() -> impl Iterator<Item = &'static str> {
        PROVIDERS.iter().map(|provider| provider.name)
    }

    pub(crate) fn provider(name: &str) -> Result<&'static Provider> {
        PROVIDERS
            .iter()
            .find(|provider| {
                // Python maps canonical SHA-3 names exactly; OpenSSL accepts its
                // hyphenated names and OIDs independently of ASCII case.
                provider.name == name
                    || (!provider.name.contains('_')
                        && !provider.name.starts_with("blake2")
                        && provider.name.eq_ignore_ascii_case(name))
                    || provider
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(name))
            })
            .ok_or_else(|| Error::new(Kind::ValueError, format!("unsupported hash type {name}")))
    }

    pub(crate) const CLASS: &'static str = "hashlib._Hash";
    pub(crate) const OPENSSL_CLASS: &'static str = "_hashlib.HASH";
    pub(crate) const XOF_CLASS: &'static str = "hashlib._HashXOF";

    pub(crate) fn public_constructor(algorithm: &str) -> Option<&'static str> {
        match algorithm {
            "blake2b" => Some("_blake2.blake2b"),
            "blake2s" => Some("_blake2.blake2s"),
            "sha3_224" => Some("_sha3.sha3_224"),
            "sha3_256" => Some("_sha3.sha3_256"),
            "sha3_384" => Some("_sha3.sha3_384"),
            "sha3_512" => Some("_sha3.sha3_512"),
            "shake_128" => Some("_sha3.shake_128"),
            "shake_256" => Some("_sha3.shake_256"),
            _ => None,
        }
    }

    pub(crate) fn provider_class(class: &str) -> Option<&'static str> {
        if class == Self::OPENSSL_CLASS {
            return Some(Self::OPENSSL_CLASS);
        }
        let (_, name) = class.rsplit_once('.')?;
        Self::public_constructor(name).filter(|&qualified| qualified == class)
    }

    pub(crate) fn is_provider_class(class: &str) -> bool {
        Self::provider_class(class).is_some()
    }

    pub(crate) fn descriptor_name(class: &str, name: &str) -> Option<&'static str> {
        match name {
            "name" => Some("name"),
            "block_size" => Some("block_size"),
            "digest_size" => Some("digest_size"),
            "_rate_bits" if class.starts_with("_sha3.") => Some("_rate_bits"),
            "_capacity_bits" if class.starts_with("_sha3.") => Some("_capacity_bits"),
            "_suffix" if class.starts_with("_sha3.") => Some("_suffix"),
            _ => None,
        }
    }

    pub(crate) fn class_constant(class: &str, attr: &str) -> Option<usize> {
        match (class, attr) {
            ("_blake2.blake2b", "SALT_SIZE" | "PERSON_SIZE") => Some(16),
            ("_blake2.blake2s", "SALT_SIZE" | "PERSON_SIZE") => Some(8),
            ("_blake2.blake2b", "MAX_DIGEST_SIZE" | "MAX_KEY_SIZE") => Some(64),
            ("_blake2.blake2s", "MAX_DIGEST_SIZE" | "MAX_KEY_SIZE") => Some(32),
            _ => None,
        }
    }

    pub(crate) fn class_name(&self) -> &'static str {
        self.class
    }

    pub(crate) fn repr(&self, identity: usize) -> impl std::fmt::Display {
        std::fmt::from_fn(move |f| {
            if Self::is_provider_class(self.class_name()) || self.class_name() == "_md5.md5" {
                return write!(f, "<{} object at {identity:#x}>", self.class_name());
            }
            write!(
                f,
                "<{} {} object @ {identity:#x}>",
                self.name(),
                self.class_name()
            )
        })
    }

    pub(crate) fn sha1(data: &[u8]) -> Self {
        Self::new(Algorithm::Sha1(Sha1::new_with_prefix(data)), None)
    }

    pub(crate) fn name(&self) -> &'static str {
        match &self.algorithm {
            Algorithm::Blake2b(_, _) => "blake2b",
            Algorithm::Blake2s(_, _) => "blake2s",
            Algorithm::Md5(_) => "md5",
            Algorithm::Sha1(_) => "sha1",
            Algorithm::Sha224(_) => "sha224",
            Algorithm::Sha256(_) => "sha256",
            Algorithm::Sha384(_) => "sha384",
            Algorithm::Sha512(_) => "sha512",
            Algorithm::Sha3_224(_) => "sha3_224",
            Algorithm::Sha3_256(_) => "sha3_256",
            Algorithm::Sha3_384(_) => "sha3_384",
            Algorithm::Sha3_512(_) => "sha3_512",
            Algorithm::Shake128(_) => "shake_128",
            Algorithm::Shake256(_) => "shake_256",
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        match &mut self.algorithm {
            Algorithm::Blake2b(h, _) => {
                h.update(data);
            }
            Algorithm::Blake2s(h, _) => {
                h.update(data);
            }
            Algorithm::Md5(h) => h.update(data),
            Algorithm::Sha1(h) => h.update(data),
            Algorithm::Sha224(h) => h.update(data),
            Algorithm::Sha256(h) => h.update(data),
            Algorithm::Sha384(h) => h.update(data),
            Algorithm::Sha512(h) => h.update(data),
            Algorithm::Sha3_224(h) => h.update(data),
            Algorithm::Sha3_256(h) => h.update(data),
            Algorithm::Sha3_384(h) => h.update(data),
            Algorithm::Sha3_512(h) => h.update(data),
            Algorithm::Shake128(h) => sha3::digest::Update::update(h, data),
            Algorithm::Shake256(h) => sha3::digest::Update::update(h, data),
        }
    }

    pub(crate) fn block_size(&self) -> usize {
        match &self.algorithm {
            Algorithm::Blake2b(_, _) => blake2b_simd::BLOCKBYTES,
            Algorithm::Blake2s(_, _) => blake2s_simd::BLOCKBYTES,
            Algorithm::Md5(_) => Md5::block_size(),
            Algorithm::Sha1(_) => Sha1::block_size(),
            Algorithm::Sha224(_) => Sha224::block_size(),
            Algorithm::Sha256(_) => Sha256::block_size(),
            Algorithm::Sha384(_) => Sha384::block_size(),
            Algorithm::Sha512(_) => Sha512::block_size(),
            Algorithm::Sha3_224(_) => Sha3_224::block_size(),
            Algorithm::Sha3_256(_) => Sha3_256::block_size(),
            Algorithm::Sha3_384(_) => Sha3_384::block_size(),
            Algorithm::Sha3_512(_) => Sha3_512::block_size(),
            Algorithm::Shake128(_) => Shake128::block_size(),
            Algorithm::Shake256(_) => Shake256::block_size(),
        }
    }

    pub(crate) fn digest_size(&self) -> usize {
        match &self.algorithm {
            Algorithm::Blake2b(_, n) | Algorithm::Blake2s(_, n) => *n,
            Algorithm::Md5(_) => Md5::output_size(),
            Algorithm::Sha1(_) => Sha1::output_size(),
            Algorithm::Sha224(_) => Sha224::output_size(),
            Algorithm::Sha256(_) => Sha256::output_size(),
            Algorithm::Sha384(_) => Sha384::output_size(),
            Algorithm::Sha512(_) => Sha512::output_size(),
            Algorithm::Sha3_224(_) => Sha3_224::output_size(),
            Algorithm::Sha3_256(_) => Sha3_256::output_size(),
            Algorithm::Sha3_384(_) => Sha3_384::output_size(),
            Algorithm::Sha3_512(_) => Sha3_512::output_size(),
            Algorithm::Shake128(_) | Algorithm::Shake256(_) => 0,
        }
    }

    pub(crate) fn fixed_digest<T>(
        &self,
        method: &str,
        result: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        match &self.algorithm {
            Algorithm::Blake2b(h, _) => result(h.finalize().as_bytes()),
            Algorithm::Blake2s(h, _) => result(h.finalize().as_bytes()),
            Algorithm::Md5(h) => result(&h.clone().finalize()),
            Algorithm::Sha1(h) => result(&h.clone().finalize()),
            Algorithm::Sha224(h) => result(&h.clone().finalize()),
            Algorithm::Sha256(h) => result(&h.clone().finalize()),
            Algorithm::Sha384(h) => result(&h.clone().finalize()),
            Algorithm::Sha512(h) => result(&h.clone().finalize()),
            Algorithm::Sha3_224(h) => result(&h.clone().finalize()),
            Algorithm::Sha3_256(h) => result(&h.clone().finalize()),
            Algorithm::Sha3_384(h) => result(&h.clone().finalize()),
            Algorithm::Sha3_512(h) => result(&h.clone().finalize()),
            Algorithm::Shake128(_) | Algorithm::Shake256(_) => Err(Error::new(
                Kind::TypeError,
                format!("{method}() missing required argument 'length' (pos 1)"),
            )),
        }
    }

    // Match the non-OpenSSL SHAKE provider's unsigned-long conversion and squeeze limit.
    pub(crate) fn xof_digest(&self, length: &Int) -> Result<Bytes> {
        let length = length.big();
        if length.is_negative() {
            return Err(Error::new(Kind::ValueError, "Cannot convert negative int"));
        }
        let length = length.to_u64().ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large for C unsigned long",
            )
        })?;
        if length >= 1 << 29 {
            return Err(Error::new(Kind::ValueError, "length is too large"));
        }
        let mut bytes = super::bytes::allocate(length as usize)?;
        bytes.resize(length as usize, 0);
        match &self.algorithm {
            Algorithm::Shake128(h) => h.clone().finalize_xof_into(&mut bytes),
            Algorithm::Shake256(h) => h.clone().finalize_xof_into(&mut bytes),
            _ => unreachable!("checked XOF receiver"),
        }
        Ok(bytes)
    }
}

fn handle(state: DigestState) -> Result<*const Object> {
    let state = allocator_api2::boxed::Box::try_new(state).map_err(|_| Error::allocation())?;
    ObjectOwner::try_new(Object(StorageCell::new(Storage::Hash(state))))
        .map(ObjectOwner::into_raw)
        .map_err(|_| Error::allocation())
}

/// # Safety
/// The caller roots the exact str and bytes until the constructor returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_new(
    name: *const Str,
    data: *const Bytes,
    used_for_security: bool,
) -> *const Object {
    // SAFETY: both operands remain owned for this operation.
    let (name, data) = unsafe { (&*name, &*data) };
    let result = DigestState::public(name, data, used_for_security).and_then(handle);
    finish(result, ptr::null())
}

thread_local! {
    static AVAILABLE: RefCell<Option<ObjectOwner>> = const { RefCell::new(None) };
    static GUARANTEED: RefCell<Option<ObjectOwner>> = const { RefCell::new(None) };
}

fn algorithms(root: &'static std::thread::LocalKey<RefCell<Option<ObjectOwner>>>) -> *const Object {
    let result = root.with_borrow_mut(|slot| {
        let set = match slot {
            Some(set) => set,
            None => slot.insert(super::mappings::Set::names(State::algorithms(), false)?),
        };
        Ok(ObjectOwner::into_raw(ObjectOwner::clone(set)))
    });
    finish(result, ptr::null())
}

/// The public sets have independent, stable identities. Mutating them does not enable a provider.
#[unsafe(no_mangle)]
pub extern "C" fn imre_hash_algorithms_available() -> *const Object {
    algorithms(&AVAILABLE)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_hash_algorithms_guaranteed() -> *const Object {
    algorithms(&GUARANTEED)
}

pub(super) fn clear_algorithms() {
    AVAILABLE.with(RefCell::take);
    GUARANTEED.with(RefCell::take);
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Encode each input byte directly into two digits of the final ASCII string.
pub(crate) fn hex_text(bytes: &[u8]) -> Result<Str> {
    let length = bytes.len().checked_mul(2).ok_or_else(Error::allocation)?;
    let mut text = Str::try_with_capacity(length).map_err(|_| Error::allocation())?;
    // SAFETY: every write is an ASCII digit within the reserved capacity.
    let space = unsafe { text.spare_capacity_mut() };
    write_hex_digits(bytes, b'a' - 10, &mut space[..length]);
    // SAFETY: the loop initialized exactly two valid UTF-8 bytes per input byte.
    unsafe { text.set_len(length) };
    Ok(text)
}

/// Encode contiguous input directly into the returned uppercase byte buffer.
pub(crate) fn uppercase_hex_bytes(input: &[u8]) -> Result<Vec<u8>> {
    hex_bytes(input, b'A' - 10)
}

fn hex_bytes(input: &[u8], letter_offset: u8) -> Result<Vec<u8>> {
    let length = input.len().checked_mul(2).ok_or_else(Error::allocation)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| Error::allocation())?;
    write_hex_digits(
        input,
        letter_offset,
        &mut output.spare_capacity_mut()[..length],
    );
    // SAFETY: each input byte initialized exactly two output bytes.
    unsafe { output.set_len(length) };
    Ok(output)
}

/// Write into the final buffer for either text or bytes, using one vectorizable loop.
#[inline(never)]
fn write_hex_digits(input: &[u8], letter_offset: u8, output: &mut [std::mem::MaybeUninit<u8>]) {
    let (pairs, _) = output.as_chunks_mut::<2>();
    let digit = |nibble| {
        if nibble < 10 {
            b'0' + nibble
        } else {
            letter_offset + nibble
        }
    };
    for (&byte, pair) in input.iter().zip(pairs) {
        pair[0].write(digit(byte >> 4));
        pair[1].write(digit(byte & 15));
    }
}

pub(crate) fn hex_with_separator(
    bytes: &[u8],
    separator: Option<u8>,
    group: i32,
) -> Result<Vec<u8>> {
    let step = group.unsigned_abs() as usize;
    let Some(separator) = separator.filter(|_| step > 0 && step < bytes.len()) else {
        return hex_bytes(bytes, b'a' - 10);
    };
    let separators = (bytes.len() - 1) / step;
    let capacity = bytes
        .len()
        .checked_mul(2)
        .and_then(|n| n.checked_add(separators))
        .ok_or_else(Error::allocation)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(capacity)
        .map_err(|_| Error::allocation())?;
    let first = if group > 0 {
        (bytes.len() - 1) % step + 1
    } else {
        step
    };
    let output = &mut result.spare_capacity_mut()[..capacity];
    let (first_output, rest) = output.split_at_mut(2 * first);
    write_hex_digits(&bytes[..first], b'a' - 10, first_output);
    let remaining = &bytes[first..];
    if step == 1 {
        // One separator and two digits have a fixed stride for every input byte.
        let (triples, _) = rest.as_chunks_mut::<3>();
        for (triple, &byte) in triples.iter_mut().zip(remaining) {
            triple[0].write(separator);
            triple[1].write(HEX[usize::from(byte >> 4)]);
            triple[2].write(HEX[usize::from(byte & 15)]);
        }
    } else {
        for (destination, source) in rest.chunks_mut(2 * step + 1).zip(remaining.chunks(step)) {
            destination[0].write(separator);
            write_hex_digits(source, b'a' - 10, &mut destination[1..]);
        }
    }
    // SAFETY: the first group writes two digits per byte. Each remaining group
    // also writes its leading separator, including a shorter last group.
    unsafe { result.set_len(capacity) };
    Ok(result)
}

fn hex(digest: &[u8]) -> Result<*const Str> {
    crate::checked_owned_str(hex_text(digest)?)
}

/// # Safety
/// The caller roots the immutable bytes until the one-shot digest completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha1_hex(value: *const Bytes) -> *const Str {
    // SAFETY: the compiler roots the exact bytes until hashing returns.
    finish(hex(&Sha1::digest(unsafe { &*value })), ptr::null())
}

/// # Safety
/// The caller roots the immutable bytes until the one-shot digest completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_hex(value: *const Bytes) -> *const Str {
    // SAFETY: the compiler roots the exact bytes until hashing returns.
    finish(hex(&Sha256::digest(unsafe { &*value })), ptr::null())
}

/// # Safety
/// The caller roots the initial bytes. The new hash owns its state independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha1_new(value: *const Bytes) -> *const Object {
    // SAFETY: the provider consumes the borrowed bytes before returning.
    finish(
        handle(DigestState::Portable(State::sha1(unsafe { &*value }))),
        ptr::null(),
    )
}

/// # Safety
/// The caller roots the initial bytes. The new hash owns its state independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_new(value: *const Bytes) -> *const Object {
    // SAFETY: the provider consumes the borrowed bytes before returning.
    finish(
        handle(DigestState::Portable(State::new(
            Algorithm::Sha256(Sha256::new_with_prefix(unsafe { &*value })),
            None,
        ))),
        ptr::null(),
    )
}

/// # Safety
/// Both typed handles are rooted. The state update does not invoke Python code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_update(hash: *const Object, value: *const Bytes) {
    // SAFETY: the exclusive state access cannot reenter, and no input reference is stored.
    let result = unsafe {
        object(hash).0.with_mut(|storage| {
            let Storage::Hash(state) = storage else {
                unreachable!()
            };
            state.update_native(&*value)
        })
    };
    finish(result, ());
}

/// Read through the buffered stream so its pending bytes and cursor are respected.
fn update_buffered_file(
    state: &mut DigestState,
    file: &mut super::FileState,
    size: &Int,
) -> Result<()> {
    match file.binary_type_name() {
        "_io.BytesIO" => {
            return state.update_native(&file.memory_value()?);
        }
        "_io.BufferedReader" | "_io.BufferedRandom" => {}
        "_io.BufferedWriter" => {
            let Some(path) = &file.name else {
                return Err(Error::new(
                    Kind::NotImplementedError,
                    "file_digest does not support an unnamed buffered writer",
                ));
            };
            let name = super::exceptions::string_repr(path);
            return Err(Error::new(
                Kind::ValueError,
                format!(
                    "'<_io.BufferedWriter name={name}>' is not a file-like object in binary reading mode."
                ),
            ));
        }
        _ => {
            return Err(Error::new(
                Kind::NotImplementedError,
                "file_digest requires a buffered binary file or BytesIO",
            ));
        }
    }
    if file.closed() {
        return Err(Error::new(Kind::ValueError, "I/O operation on closed file"));
    }
    let size = usize::try_from(super::bytes::native_size(size)?)
        .map_err(|_| Error::new(Kind::ValueError, "negative count"))?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(size)
        .map_err(|_| Error::memory_error())?;
    buffer.resize(size, 0);
    loop {
        match file.read_into(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => state.update_native(&buffer[..count])?,
            Err(error) => match error.os_args().map(|args| args.errno) {
                Some(code) if code == i64::from(libc::EINTR) => continue,
                Some(code)
                    if code == i64::from(libc::EAGAIN) || code == i64::from(libc::EWOULDBLOCK) =>
                {
                    return Err(Error::new(
                        Kind::BlockingIOError,
                        "I/O operation would block.",
                    ));
                }
                _ => return Err(error),
            },
        }
    }
}

/// Update an initialized digest from an exact native binary stream.
///
/// # Safety
/// Both distinct typed handles and the tagged size are rooted throughout. Reading
/// the native stream and updating the digest cannot call Python or reenter either owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_update_file(
    hash: *const Object,
    file: *const Object,
    size: usize,
) {
    // SAFETY: the compiler supplies an exact file handle with stable class metadata.
    if unsafe { super::files::imre_file_is_raw(file) } != 0 {
        Error::new(
            Kind::NotImplementedError,
            "file_digest does not support raw FileIO",
        )
        .raise();
        return;
    }
    // SAFETY: both live, distinct handles are borrowed exclusively in these callbacks.
    let result = unsafe {
        let size = Int::borrow(size);
        object(hash).0.with_mut(|storage| {
            let Storage::Hash(state) = storage else {
                unreachable!()
            };
            object(file)
                .0
                .with_mut(|stream| update_buffered_file(state, stream.file_mut(), &size))
        })
    };
    finish(result, ());
}

/// # Safety
/// The caller roots an exact hash object and the operation cannot invoke Python code.
unsafe fn with_state<T>(hash: *const Object, f: impl FnOnce(&DigestState) -> T) -> T {
    // SAFETY: the rooted hash remains immutably borrowed for the callback.
    unsafe {
        object(hash).0.with(|storage| {
            let Storage::Hash(state) = storage else {
                unreachable!()
            };
            f(state)
        })
    }
}

/// # Safety
/// The compiler roots the hash while selecting its interned nominal class name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_type(hash: *const Object) -> *const Str {
    // SAFETY: the name is static and does not borrow storage from the rooted handle.
    let name = unsafe { with_state(hash, DigestState::class_name) };
    let class = crate::StrOwner::try_new(Str::const_new(name))
        .ok()
        .and_then(|name| crate::sys::try_intern_owner(&name))
        .map(crate::StrOwner::into_raw)
        .ok_or_else(Error::allocation);
    finish(class, ptr::null())
}

/// The address is stable while an alias retains the rooted native hash.
#[unsafe(no_mangle)]
pub extern "C" fn imre_hash_identity(hash: *const Object) -> usize {
    super::imre_pointer_identity(hash.cast())
}

/// # Safety
/// The compiler roots the hash until its provider name has been copied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_name(hash: *const Object) -> *const Str {
    // SAFETY: provider names are static strings independent of the hash.
    let name = unsafe { with_state(hash, DigestState::name) };
    finish(crate::checked_owned_str(Str::from(name)), ptr::null())
}

/// # Safety
/// The compiler roots the exact hash during the metadata lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_digest_size(hash: *const Object) -> usize {
    // SAFETY: the lookup only reads the hash's algorithm tag.
    let size = unsafe { with_state(hash, DigestState::digest_size) };
    Int::from_usize(size).into_raw()
}

/// # Safety
/// The compiler roots the exact hash during the metadata lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_block_size(hash: *const Object) -> usize {
    // SAFETY: the lookup only reads the hash's algorithm tag.
    let size = unsafe { with_state(hash, DigestState::block_size) };
    Int::from_usize(size).into_raw()
}

/// # Safety
/// The caller roots the hash. Its independent copy can be updated separately.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_copy(hash: *const Object) -> *const Object {
    // SAFETY: the provider copies the rooted streaming state before returning.
    finish(
        unsafe { with_state(hash, DigestState::try_copy) }.and_then(handle),
        ptr::null(),
    )
}

/// # Safety
/// The caller roots the hash. The returned digest owns its bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_digest(hash: *const Object) -> *const Bytes {
    // SAFETY: finalize reads a snapshot of the rooted hash.
    let digest = unsafe {
        with_state(hash, |s| {
            s.fixed_digest("digest", |bytes| match bytes {
                Cow::Borrowed(bytes) => super::bytes::owned_input(bytes),
                Cow::Owned(bytes) => BytesOwner::try_new(bytes).map_err(|_| Error::memory_error()),
            })
        })
    };
    finish(digest.map(BytesOwner::into_raw), ptr::null())
}

/// # Safety
/// The caller roots the hash. The lowercase hexadecimal result owns its storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_hexdigest(hash: *const Object) -> *const Str {
    // SAFETY: finalize reads a snapshot of the rooted hash.
    let result = unsafe {
        with_state(hash, |s| {
            s.fixed_digest("hexdigest", |bytes| hex_text(&bytes))
        })
    };
    finish(result.and_then(crate::checked_owned_str), ptr::null())
}

/// # Safety
/// The compiler roots the XOF handle and the exact integer until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_xof_digest(hash: *const Object, length: usize) -> *const Bytes {
    // SAFETY: the checked handle and tagged integer remain owned by the caller.
    let bytes = unsafe { with_state(hash, |s| s.xof("digest", &Int::borrow(length))) };
    finish(
        bytes
            .and_then(|bytes| BytesOwner::try_new(bytes).map_err(|_| Error::memory_error()))
            .map(BytesOwner::into_raw),
        ptr::null(),
    )
}

/// # Safety
/// The compiler roots the XOF handle and the exact integer until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_xof_hexdigest(hash: *const Object, length: usize) -> *const Str {
    // SAFETY: the checked handle and tagged integer remain owned by the caller.
    let text = unsafe {
        with_state(hash, |s| {
            s.xof("hexdigest", &Int::borrow(length))
                .and_then(|bytes| hex_text(&bytes))
        })
    };
    finish(text.and_then(crate::checked_owned_str), ptr::null())
}
