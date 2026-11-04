//! Constants shared by the checked and runtime zlib modules.

/// Compatibility version returned by the pinned zlib-rs 0.6.8 C API's `zlibVersion()`.
pub const VERSION: &str = "1.3.0-zlib-rs-0.6.8";

pub fn integer_constant(name: &str) -> Option<i64> {
    Some(match name {
        "MAX_WBITS" => 15,
        "DEFLATED" => 8,
        "DEF_MEM_LEVEL" => 8,
        "DEF_BUF_SIZE" => 16 * 1024,
        "Z_NO_COMPRESSION" => 0,
        "Z_BEST_SPEED" => 1,
        "Z_BEST_COMPRESSION" => 9,
        "Z_DEFAULT_COMPRESSION" => -1,
        "Z_DEFAULT_STRATEGY" => 0,
        "Z_FILTERED" => 1,
        "Z_HUFFMAN_ONLY" => 2,
        "Z_RLE" => 3,
        "Z_FIXED" => 4,
        "Z_NO_FLUSH" => 0,
        "Z_PARTIAL_FLUSH" => 1,
        "Z_SYNC_FLUSH" => 2,
        "Z_FULL_FLUSH" => 3,
        "Z_FINISH" => 4,
        "Z_BLOCK" => 5,
        "Z_TREES" => 6,
        _ => return None,
    })
}
