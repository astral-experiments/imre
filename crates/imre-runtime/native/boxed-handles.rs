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

/// Authenticated bundled difflib definition tags; zero remains an ordinary native body.
pub(crate) const DIFFLIB_INIT_BODY: u32 = 1;
pub(crate) const DIFFLIB_SET_SEQS_BODY: u32 = 2;
pub(crate) const DIFFLIB_SET_SEQ1_BODY: u32 = 3;
pub(crate) const DIFFLIB_SET_SEQ2_BODY: u32 = 4;
pub(crate) const DIFFLIB_CHAIN_B_BODY: u32 = 5;
pub(crate) const DIFFLIB_FIND_BODY: u32 = 6;
pub(crate) const DIFFLIB_GET_BLOCKS_BODY: u32 = 7;
pub(crate) const COLLECTIONS_NAMEDTUPLE_MAKE_BODY: u32 = 8;
pub(crate) const DIFFLIB_RATIO_BODY: u32 = 9;
pub(crate) const DIFFLIB_CALCULATE_RATIO_BODY: u32 = 10;

/// Authenticated bundled pickle reader definitions.
pub(crate) const PICKLE_LOADS_BODY: u32 = 11;

/// Authenticated bundled pathlib definitions used by whole-file reads.
pub(crate) const PATH_READ_BYTES_BODY: u32 = 100;
pub(crate) const PATH_OPEN_BODY: u32 = 101;
pub(crate) const PATH_FSPATH_BODY: u32 = 102;
pub(crate) const PATH_STR_BODY: u32 = 103;
pub(crate) const PATH_PARSE_BODY: u32 = 104;
pub(crate) const PATH_FORMAT_BODY: u32 = 105;
pub(crate) const PATH_RAW_BODY: u32 = 106;
pub(crate) const PATH_DRIVE_BODY: u32 = 107;
pub(crate) const PATH_ROOT_BODY: u32 = 108;
pub(crate) const PATH_TAIL_BODY: u32 = 109;
