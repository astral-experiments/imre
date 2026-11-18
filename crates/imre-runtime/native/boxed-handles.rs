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
pub(crate) const PICKLE_READER_INIT_BODY: u32 = 12;
pub(crate) const PICKLE_READER_LOAD_BODY: u32 = 13;
pub(crate) const PICKLE_REQUIRE_STACK_BODY: u32 = 14;
pub(crate) const PICKLE_POP_MARK_BODY: u32 = 15;
pub(crate) const PICKLE_APPEND_ITEMS_BODY: u32 = 16;
pub(crate) const PICKLE_PROTO_BODY: u32 = 17;
pub(crate) const PICKLE_FRAME_BODY: u32 = 18;
pub(crate) const PICKLE_EMPTY_LIST_BODY: u32 = 19;
pub(crate) const PICKLE_MARK_BODY: u32 = 20;
pub(crate) const PICKLE_SHORT_UNICODE_BODY: u32 = 21;
pub(crate) const PICKLE_MEMOIZE_BODY: u32 = 22;
pub(crate) const PICKLE_APPENDS_BODY: u32 = 23;
pub(crate) const PICKLE_STOP_BODY: u32 = 24;
pub(crate) const PICKLE_BINGET_BODY: u32 = 25;
pub(crate) const PICKLE_LONG_BINGET_BODY: u32 = 26;
pub(crate) const PICKLE_UNFRAMER_INIT_BODY: u32 = 27;
pub(crate) const PICKLE_UNFRAMER_READ_BODY: u32 = 28;
pub(crate) const PICKLE_UNFRAMER_EXACT_BODY: u32 = 29;
pub(crate) const PICKLE_UNFRAMER_LINE_BODY: u32 = 30;
pub(crate) const PICKLE_UNFRAMER_FRAME_BODY: u32 = 31;
pub(crate) const PICKLE_BYTES_FRAME_INIT_BODY: u32 = 32;
pub(crate) const PICKLE_BYTES_FRAME_READ_BODY: u32 = 33;
