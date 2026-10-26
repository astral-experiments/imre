//! Module-level random draws share one MT19937 state, as in CPython 3.14.
//! The original generator and Python semantics are distributed under the notices
//! in licenses/MT19937.txt and licenses/CPython.txt.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{ToPrimitive, Zero};
use sha2::{Digest, Sha512};

use super::exceptions::{Error, Kind, Result, finish};
use super::{Bytes, Int, Str};

const N: usize = 624;
const M: usize = 397;

struct Generator {
    state: [u32; N],
    index: usize,
}

impl Generator {
    fn from_key(key: &[u32]) -> Self {
        let key = if key.is_empty() { &[0] } else { key };
        let mut state = [0u32; N];
        state[0] = 19_650_218;
        let mix = |x: u32, factor| (x ^ (x >> 30)).wrapping_mul(factor);
        for i in 1..N {
            state[i] = mix(state[i - 1], 1_812_433_253).wrapping_add(i as u32);
        }
        let mut i = 1;
        for j in 0..N.max(key.len()) {
            let j = j % key.len();
            state[i] = (state[i] ^ mix(state[i - 1], 1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            if i == N {
                state[0] = state[N - 1];
                i = 1;
            }
        }
        for _ in 1..N {
            state[i] = (state[i] ^ mix(state[i - 1], 1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i == N {
                state[0] = state[N - 1];
                i = 1;
            }
        }
        state[0] = 1 << 31;
        Self { state, index: N }
    }

    fn from_entropy() -> Self {
        let mut seed = [0; N * 4];
        if getrandom::fill(&mut seed).is_ok() {
            return Self::from_key(&std::array::from_fn::<_, N, _>(|i| {
                u32::from_ne_bytes(seed[i * 4..i * 4 + 4].try_into().unwrap())
            }));
        }
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        #[cfg(unix)]
        let mono = {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: clock_gettime writes to this live timespec.
            unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) };
            (value.tv_sec as u64)
                .wrapping_mul(1_000_000_000)
                .wrapping_add(value.tv_nsec as u64)
        };
        #[cfg(not(unix))]
        let mono = {
            use std::time::Instant;
            static START: OnceLock<Instant> = OnceLock::new();
            START.get_or_init(Instant::now).elapsed().as_nanos() as u64
        };
        Self::from_key(&[
            wall as u32,
            (wall >> 32) as u32,
            std::process::id(),
            mono as u32,
            (mono >> 32) as u32,
        ])
    }

    fn word(&mut self) -> u32 {
        if self.index == N {
            for i in 0..N {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % N] & 0x7fff_ffff);
                self.state[i] =
                    self.state[(i + M) % N] ^ (y >> 1) ^ (0u32.wrapping_sub(y & 1) & 0x9908_b0df);
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    fn bits(&mut self, count: u64) -> Result<BigUint> {
        if count == 0 {
            return Ok(BigUint::zero());
        }
        if count <= 32 {
            return Ok(BigUint::from(self.word() >> (32 - count)));
        }
        let n = usize::try_from(count.div_ceil(32)).map_err(|_| Error::memory_error())?;
        let mut limbs = Vec::new();
        limbs
            .try_reserve_exact(n)
            .map_err(|_| Error::memory_error())?;
        for _ in 1..n {
            limbs.push(self.word());
        }
        limbs.push(self.word() >> ((32 - count % 32) % 32));
        Ok(BigUint::new(limbs))
    }

    fn below(&mut self, stop: &BigUint) -> Result<BigUint> {
        debug_assert!(!stop.is_zero());
        loop {
            let value = self.bits(stop.bits())?;
            if value < *stop {
                return Ok(value);
            }
        }
    }
}

fn shared() -> MutexGuard<'static, Generator> {
    static STATE: OnceLock<Mutex<Generator>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(Generator::from_entropy()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_random_seed_none() {
    *shared() = Generator::from_entropy();
}

/// # Safety
/// The seed must be a live tagged Python integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_random_seed_int(seed: usize) {
    // SAFETY: the compiler roots this Python integer for the call.
    let seed = unsafe { Int::borrow(seed) };
    *shared() = Generator::from_key(&seed.big().magnitude().to_u32_digits());
}

fn seed_bytes(bytes: &[u8]) {
    let mut data = bytes.to_vec();
    data.extend_from_slice(&Sha512::digest(bytes));
    *shared() = Generator::from_key(&BigUint::from_bytes_be(&data).to_u32_digits());
}

/// # Safety
/// The seed must point to a live native string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_random_seed_str(seed: *const Str) {
    // SAFETY: the compiler roots this string for the call.
    seed_bytes(unsafe { &*seed }.as_bytes());
}

/// # Safety
/// The seed must point to a live native byte vector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_random_seed_bytes(seed: *const Bytes) {
    // SAFETY: the compiler roots these bytes for the call.
    seed_bytes(unsafe { &*seed });
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_random_float() -> f64 {
    let mut rng = shared();
    let high = u64::from(rng.word() >> 5);
    let low = u64::from(rng.word() >> 6);
    ((high << 26) | low) as f64 / (1u64 << 53) as f64
}

fn integer_result(result: Result<BigInt>) -> usize {
    finish(result.map(|v| Int::new(v).into_raw()), 1)
}

/// # Safety
/// The bit count must be a live tagged Python integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_random_bits(count: usize) -> usize {
    // SAFETY: the compiler roots this Python integer for the call.
    let count = unsafe { Int::borrow(count) };
    let count = count.big();
    integer_result(if count.sign() == Sign::Minus {
        Err(Error::new(Kind::ValueError, "Cannot convert negative int"))
    } else if let Some(n) = count.to_u64() {
        shared().bits(n).map(BigInt::from)
    } else {
        Err(Error::new(
            Kind::OverflowError,
            "Python int too large for C uint64_t",
        ))
    })
}

fn sequence_index(length: &BigUint, secure: bool) -> Result<BigUint> {
    if length.is_zero() {
        return Err(Error::new(
            Kind::IndexError,
            "Cannot choose from an empty sequence",
        ));
    }
    if length.bits() >= u64::from(usize::BITS) {
        return Err(Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C ssize_t",
        ));
    }
    if !secure {
        return shared().below(length);
    }
    let limit = length.to_u64().expect("sequence length fits Py_ssize_t");
    // The interval [threshold, 2^64) contains an equal number of each residue.
    let threshold = limit.wrapping_neg() % limit;
    loop {
        let draw = getrandom::u64().map_err(|error| super::files::errno_error(error.into()))?;
        if draw >= threshold {
            return Ok(BigUint::from(draw % limit));
        }
    }
}

/// Sample a native sequence index after its length has been checked.
/// # Safety
/// The length must be a live, nonnegative Python integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_random_choice_index(length: usize) -> usize {
    // SAFETY: the compiler roots the result of len().
    let length = unsafe { Int::borrow(length) };
    integer_result(sequence_index(length.big().magnitude(), false).map(BigInt::from))
}

/// Draw an unbiased index using the system's cryptographic random source.
/// # Safety
/// The length must be a live, nonnegative Python integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_secrets_choice_index(length: usize) -> usize {
    // SAFETY: the compiler roots the result of len().
    let length = unsafe { Int::borrow(length) };
    integer_result(sequence_index(length.big().magnitude(), true).map(BigInt::from))
}

/// Select from an arithmetic progression without materializing it. A range used
/// as a sequence must fit len(); randrange and randint admit arbitrary widths.
fn progression(start: &BigInt, stop: &BigInt, step: &BigInt, mode: u32) -> Result<BigInt> {
    if mode == 3 && *step != BigInt::from(1u32) {
        return Err(Error::new(
            Kind::TypeError,
            "Missing a non-None stop argument",
        ));
    }
    if mode == 1 {
        if start > stop {
            return Err(Error::new(
                Kind::ValueError,
                format!("empty range in randint({start}, {stop})"),
            ));
        }
        return Ok(start + BigInt::from(shared().below((stop - start + 1u32).magnitude())?));
    }
    let sequence = matches!(mode, 2 | 4);
    if step.is_zero() {
        return Err(Error::new(
            Kind::ValueError,
            if sequence {
                "range() arg 3 must not be zero"
            } else {
                "zero step for randrange()"
            },
        ));
    }
    let (lo, hi) = if step.sign() == Sign::Minus {
        (stop, start)
    } else {
        (start, stop)
    };
    if hi <= lo {
        if sequence {
            return Err(Error::new(
                Kind::IndexError,
                "Cannot choose from an empty sequence",
            ));
        }
        let message = if mode == 3 {
            "empty range for randrange()".to_string()
        } else if *step == BigInt::from(1u32) {
            format!("empty range in randrange({start}, {stop})")
        } else {
            format!("empty range in randrange({start}, {stop}, {step})")
        };
        return Err(Error::new(Kind::ValueError, message));
    }
    let count = (hi - lo - 1u32).magnitude() / step.magnitude() + 1u32;
    let index = if sequence {
        sequence_index(&count, mode == 4)?
    } else {
        shared().below(&count)?
    };
    Ok(start + step * BigInt::from(index))
}

/// # Safety
/// The bounds and step must be live Python integers; the mode selects the checked public operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_random_progression(
    start: usize,
    stop: usize,
    step: usize,
    mode: u32,
) -> usize {
    // SAFETY: the compiler roots the three integer operands in source order.
    let (start, stop, step) = unsafe { (Int::borrow(start), Int::borrow(stop), Int::borrow(step)) };
    integer_result(progression(&start.big(), &stop.big(), &step.big(), mode))
}
