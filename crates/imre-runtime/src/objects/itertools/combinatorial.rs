//! Materialize each input once. Only indices and the current output row grow
//! with the requested tuple length; future result tuples are never retained.

use super::*;

enum Order {
    Product,
    Permutations { cycles: Vec<usize> },
    Combinations { replacement: bool },
}

pub(in crate::objects) struct Cursor {
    pools: Vec<Vec<OwnedWord>>,
    indices: Vec<usize>,
    width: usize,
    order: Order,
    done: bool,
}

impl Cursor {
    pub(super) fn name(&self) -> &'static str {
        match self.order {
            Order::Product => "itertools.product",
            Order::Permutations { .. } => "itertools.permutations",
            Order::Combinations { replacement: false } => "itertools.combinations",
            Order::Combinations { replacement: true } => "itertools.combinations_with_replacement",
        }
    }

    pub(super) fn next(&mut self) -> ItemResult {
        if self.done {
            return Ok(None);
        }
        // An allocation failure terminates this iterator just like exhaustion.
        self.done = true;
        let row = records::tuples::tuple_from_words(
            self.indices[..self.width]
                .iter()
                .enumerate()
                .map(|(i, &index)| self.pools[i % self.pools.len()][index].clone()),
        )?;
        self.done = !self.advance();
        Ok(Some(row))
    }

    fn advance(&mut self) -> bool {
        let n = self.pools.first().map_or(0, Vec::len);
        match &mut self.order {
            Order::Product => {
                for (pos, index) in self.indices.iter_mut().enumerate().rev() {
                    *index += 1;
                    if *index < self.pools[pos % self.pools.len()].len() {
                        return true;
                    }
                    *index = 0;
                }
            }
            Order::Permutations { cycles } => {
                for (i, cycle) in cycles.iter_mut().enumerate().rev() {
                    *cycle -= 1;
                    if *cycle == 0 {
                        self.indices[i..].rotate_left(1);
                        *cycle = n - i;
                    } else {
                        self.indices.swap(i, n - *cycle);
                        return true;
                    }
                }
            }
            Order::Combinations { replacement } => {
                for i in (0..self.width).rev() {
                    let last = if *replacement {
                        n - 1
                    } else {
                        n - self.width + i
                    };
                    if self.indices[i] < last {
                        let start = self.indices[i] + 1;
                        for (offset, index) in self.indices[i..].iter_mut().enumerate() {
                            *index = if *replacement { start } else { start + offset };
                        }
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn index_vec(size: usize, values: impl Iterator<Item = usize>) -> Result<Vec<usize>, Error> {
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(size)
        .map_err(|_| Error::memory_error())?;
    indices.extend(values.take(size));
    Ok(indices)
}

/// # Safety
/// The compiler passes one rooted source word per byte of iterable mode.
/// The parameter is a rooted canonical integer or zero for the default length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_combinatorial(
    op: u32,
    count: usize,
    modes: *const u8,
    words: *const usize,
    parameter: usize,
) -> *const Object {
    // SAFETY: both slices cover the same rooted arguments throughout this call.
    let sources = unsafe {
        std::slice::from_raw_parts(modes, count)
            .iter()
            .zip(std::slice::from_raw_parts(words, count))
    };
    let parameter = || -> Result<Option<i64>, Error> {
        // SAFETY: a nonzero parameter is an independently rooted int.
        (parameter != 0)
            .then(|| ssize(&*unsafe { Int::borrow(parameter) }))
            .transpose()
    };
    let result = (|| {
        let mut pools = Vec::new();
        let product_indices = if op == 0 {
            let repeat = usize::try_from(parameter()?.unwrap())
                .map_err(|_| Error::new(Kind::ValueError, "repeat argument cannot be negative"))?;
            let width = count
                .checked_mul(repeat)
                .filter(|&n| n <= isize::MAX as usize / size_of::<usize>())
                .ok_or_else(|| Error::new(Kind::OverflowError, "repeat argument too large"))?;
            Some(index_vec(width, std::iter::repeat(0))?)
        } else {
            None
        };
        // combinations converts its index at binding; permutations converts r after pooling.
        let length = if op >= 2 { parameter()? } else { None };
        if !product_indices.as_ref().is_some_and(Vec::is_empty) {
            pools
                .try_reserve_exact(count)
                .map_err(|_| Error::memory_error())?;
            for (&mode, &word) in sources {
                // SAFETY: the source's representation matches its compiler-selected mode.
                pools.push(unsafe { sources::collect(u32::from(mode), word) }?);
            }
        }
        let n = pools.first().map_or(0, Vec::len);
        let width = match &product_indices {
            Some(indices) => indices.len(),
            None => {
                usize::try_from(if op == 1 { parameter()? } else { length }.unwrap_or(n as i64))
                    .map_err(|_| Error::new(Kind::ValueError, "r must be non-negative"))?
            }
        };
        let (order, indices, done) = match op {
            0 => (
                Order::Product,
                product_indices.expect("product indices"),
                pools.iter().any(Vec::is_empty),
            ),
            1 => {
                let cycles = index_vec(width, (0..width).map(|i| n.saturating_sub(i)))?;
                (
                    Order::Permutations { cycles },
                    index_vec(n, 0..)?,
                    width > n,
                )
            }
            2 => (
                Order::Combinations { replacement: false },
                index_vec(width, 0..)?,
                width > n,
            ),
            3 => (
                Order::Combinations { replacement: true },
                index_vec(width, std::iter::repeat(0))?,
                n == 0 && width != 0,
            ),
            _ => unreachable!("compiler-selected ordering"),
        };
        Ok::<_, Error>(Cursor {
            pools,
            indices,
            width,
            order,
            done,
        })
    })();
    match result {
        Ok(cursor) => {
            crate::ObjectOwner::into_raw(adapter(Adapter::Combinatorial(RefCell::new(cursor))))
        }
        Err(error) => error.returning(std::ptr::null()),
    }
}
