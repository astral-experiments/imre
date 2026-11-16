// Permanent object handles shared by boxed LLVM emission and heap initialization.
pub(crate) const NONE: u64 = 0;
pub(crate) const FALSE: u64 = 1;
pub(crate) const TRUE: u64 = 2;
pub(crate) const ELLIPSIS: u64 = 3;
pub(crate) const NOT_IMPLEMENTED: u64 = 4;
pub(crate) const EMPTY_TUPLE: u64 = NOT_IMPLEMENTED + 1;
pub(crate) const SMALL_INT_MIN: i64 = -5;
pub(crate) const SMALL_INT_MAX: i64 = 256;
pub(crate) const FIRST_SMALL_INT: u64 = EMPTY_TUPLE + 1;

/// Return the permanent handle for an integer stored in every boxed heap.
#[inline]
pub(crate) fn small_integer_id(value: i64) -> Option<u64> {
    (SMALL_INT_MIN..=SMALL_INT_MAX)
        .contains(&value)
        .then(|| FIRST_SMALL_INT + (value - SMALL_INT_MIN) as u64)
}

/// Prelinked code-site tag for a native resumable definition.
pub(crate) const NATIVE_SUSPENDED_BODY: u32 = u32::MAX;
