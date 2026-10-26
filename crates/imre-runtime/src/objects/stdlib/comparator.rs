//! Stable sorting with a fallible native Python comparison.
use super::{List, apply_order, object};
use crate::Int;
use crate::objects::exceptions::{Error, imre_error_pending};
use crate::objects::words::WordKind;
use num_traits::Signed;
use smallvec::SmallVec;

#[derive(Clone, Copy)]
struct Callback {
    target: *const (),
    context: *const (),
    args: u32,
    transfer: Option<WordKind>,
    result: u32,
}

impl Callback {
    /// Both borrowed words belong to the private saved list for the full call.
    unsafe fn less(self, a: usize, b: usize) -> Result<bool, ()> {
        // SAFETY: the frontend checked both parameter types; LLVM passes their
        // physical ABI independently of the element's storage descriptor.
        unsafe {
            if let Some(kind) = self.transfer {
                kind.retain(a);
                kind.retain(b);
            }
            match self.args {
                0 => self.call(0_u8, 0_u8),
                1 => self.call(a != 0, b != 0),
                2 => self.call(a, b),
                3 => self.call(a as *const (), b as *const ()),
                7 => self.call(f64::from_bits(a as u64), f64::from_bits(b as u64)),
                _ => unreachable!("checked comparator argument"),
            }
        }
    }

    unsafe fn call<A: Copy>(self, a: A, b: A) -> Result<bool, ()> {
        type Function<A, R> = unsafe extern "C" fn(*const (), A, A) -> R;
        // SAFETY: argument and result types match this closed native callable's
        // adapter signature. Only a successful integer return transfers an owner.
        let less =
            unsafe {
                match self.result {
                    1 => {
                        std::mem::transmute::<*const (), Function<A, bool>>(self.target)(
                            self.context,
                            a,
                            b,
                        );
                        false // Both Python booleans compare >= 0.
                    }
                    2 => {
                        let word = std::mem::transmute::<*const (), Function<A, usize>>(
                            self.target,
                        )(self.context, a, b);
                        if imre_error_pending() != 0 {
                            return Err(());
                        }
                        let value = Int(word);
                        value
                            .as_small()
                            .map_or_else(|| value.big().is_negative(), |small| small < 0)
                    }
                    7 => {
                        std::mem::transmute::<*const (), Function<A, f64>>(self.target)(
                            self.context,
                            a,
                            b,
                        ) < 0.0
                    }
                    _ => unreachable!("checked comparator return"),
                }
            };
        if imre_error_pending() == 0 {
            Ok(less)
        } else {
            Err(())
        }
    }
}

type Indices = SmallVec<[usize; 32]>;
type Runs = SmallVec<[(usize, usize); 8]>;

/// Detect monotone runs, extend short runs with stable binary insertion, and
/// merge using only the Python less-than operation. A failed comparison never
/// leaves the index array with a duplicated or missing owner.
/// The comparison call sequence is specific to this sorting implementation.
fn sort_indices(
    order: &mut [usize],
    runs: &mut Runs,
    scratch: &mut Indices,
    min_run: usize,
    mut less: impl FnMut(usize, usize) -> Result<bool, ()>,
) -> Result<(), ()> {
    let n = order.len();
    let mut start = 0;
    while start < n {
        let mut end = start + 1;
        if end < n {
            let descending = less(order[end], order[start])?;
            end += 1;
            while end < n && less(order[end], order[end - 1])? == descending {
                end += 1;
            }
            if descending {
                order[start..end].reverse();
            }
        }
        let stop = start.saturating_add(min_run).min(n);
        while end < stop {
            let pivot = order[end];
            let (mut lo, mut hi) = (start, end);
            while lo < hi {
                let middle = lo + (hi - lo) / 2;
                if less(pivot, order[middle])? {
                    hi = middle;
                } else {
                    lo = middle + 1;
                }
            }
            order.copy_within(lo..end, lo + 1);
            order[lo] = pivot;
            end += 1;
        }
        runs.push((start, end));
        start = end;
    }
    while runs.len() > 1 {
        let mut written = 0;
        for index in (0..runs.len()).step_by(2) {
            let (lo, mid) = runs[index];
            let Some(&(_, hi)) = runs.get(index + 1) else {
                runs[written] = (lo, mid);
                written += 1;
                break;
            };
            scratch.clear();
            let (mut a, mut b) = (lo, mid);
            while a < mid && b < hi {
                if less(order[b], order[a])? {
                    scratch.push(order[b]);
                    b += 1;
                } else {
                    scratch.push(order[a]);
                    a += 1;
                }
            }
            scratch.extend_from_slice(&order[a..mid]);
            scratch.extend_from_slice(&order[b..hi]);
            order[lo..hi].copy_from_slice(scratch);
            runs[written] = (lo, hi);
            written += 1;
        }
        runs.truncate(written);
    }
    Ok(())
}

/// # Safety
/// The saved list and the exact native callback context stay rooted throughout
/// sorting. The saved list is private: callbacks can see only the detached
/// original list, or the input from which sorted() made its copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sort_by_comparator(
    list: *const (),
    kind: u32,
    target: *const (),
    context: *const (),
    args: u32,
    result: u32,
    reverse: u32,
) {
    let mut words = Indices::new();
    // SAFETY: only borrowed words escape the storage borrow. The private list,
    // still rooted by the caller, continues to own every referenced object.
    let copied: Result<(), Error> = unsafe {
        if kind == 2 {
            (&*list.cast::<List>()).0.with(|values| {
                words
                    .try_reserve_exact(values.len())
                    .map_err(|_| Error::allocation())?;
                words.extend(values.iter().map(|value| value.0));
                Ok(())
            })
        } else {
            object(list.cast()).0.with(|storage| {
                let values = storage.sequence()?;
                values.check_kind(kind);
                words
                    .try_reserve_exact(values.values.len())
                    .map_err(|_| Error::allocation())?;
                words.extend_from_slice(&values.values);
                Ok(())
            })
        }
    };
    if let Err(error) = copied {
        error.raise();
        return;
    }
    let n = words.len();
    if n < 2 {
        return;
    }
    let (mut min_run, mut extra) = (n, 0);
    while min_run >= 64 {
        extra |= min_run & 1;
        min_run >>= 1;
    }
    min_run += extra;
    let (mut order, mut scratch, mut runs) = (Indices::new(), Indices::new(), Runs::new());
    if order.try_reserve_exact(n).is_err()
        || scratch.try_reserve_exact(n).is_err()
        || runs.try_reserve_exact(n.div_ceil(min_run)).is_err()
    {
        crate::objects::exceptions::sequence_allocation_failed();
        return;
    }
    order.extend(0..n);
    if reverse != 0 {
        order.reverse();
    }
    let callback = Callback {
        target,
        context,
        args: args & 255,
        transfer: (args & 256 != 0).then(|| WordKind::new(kind)),
        result,
    };
    let _ = sort_indices(&mut order, &mut runs, &mut scratch, min_run, |a, b| {
        // SAFETY: callback ABI and operand owners were validated above.
        unsafe { callback.less(words[a], words[b]) }
    });
    if reverse != 0 {
        order.reverse();
    }
    // SAFETY: no storage borrow overlapped Python, including a failed comparison.
    unsafe { apply_order(list, kind, &mut order) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_comparisons_leave_every_original_owner_present_once() {
        let n = 143;
        let key = |i: usize| i * 37 % 17;
        let mut expected = (0..n).collect::<Vec<_>>();
        expected.sort_by_key(|&i| key(i));
        for stop in [1, 2, 17, 65, 130, 315, usize::MAX] {
            let mut order = (0..n).collect::<Vec<_>>();
            let mut seen = 0;
            let result = sort_indices(
                &mut order,
                &mut Runs::new(),
                &mut Indices::new(),
                36,
                |a, b| {
                    seen += 1;
                    if seen == stop {
                        Err(())
                    } else {
                        Ok(key(a) < key(b))
                    }
                },
            );
            if result.is_ok() {
                assert_eq!(order, expected);
            } else {
                assert_eq!(seen, stop);
                order.sort_unstable();
                assert!(order.into_iter().eq(0..n));
            }
        }
    }
}
