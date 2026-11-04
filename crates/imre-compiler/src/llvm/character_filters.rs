//! Build a SIMD membership test for a literal ASCII character class.
use std::fmt::Write;

use super::Output;

/// Emit a predicate over 16 bytes when the class consists of a few contiguous ranges.
/// The caller uses the scalar lookup table for more fragmented classes.
pub(super) fn vector_matcher(output: &mut Output, table: &[u8], id: usize) -> String {
    const MAX_RANGES: usize = 10;
    let mut ranges = [(0, 0); MAX_RANGES];
    let mut count = 0;
    let mut start = 0;
    for run in table.chunk_by(|a, b| a == b) {
        if run[0] != 0 {
            let Some(range) = ranges.get_mut(count) else {
                return "null".to_owned();
            };
            *range = (start, run.len() - 1);
            count += 1;
        }
        start += run.len();
    }
    if count == 0 {
        return "null".to_owned();
    }

    let name = format!("ascii_filter_{id}");
    let body = output.function(
        output.current,
        &name,
        "i1",
        "<16 x i8> %bytes",
        " alwaysinline",
    );
    let mut mask = "zeroinitializer".to_owned();
    for (index, &(start, span)) in ranges[..count].iter().enumerate() {
        let start = byte_splat(start);
        let span = byte_splat(span);
        // An unsigned wrapping subtraction tests both ends of the range in one comparison.
        writeln!(body, "  %offset{index} = sub <16 x i8> %bytes, {start}").unwrap();
        writeln!(
            body,
            "  %hit{index} = icmp ule <16 x i8> %offset{index}, {span}"
        )
        .unwrap();
        writeln!(body, "  %mask{index} = or <16 x i1> {mask}, %hit{index}").unwrap();
        mask = format!("%mask{index}");
    }
    writeln!(body, "  %bits = bitcast <16 x i1> {mask} to i16\n  %any = icmp ne i16 %bits, 0\n  ret i1 %any\n}}").unwrap();
    format!("@{name}")
}
/// Spell a portable LLVM constant with the same byte in every lane.
fn byte_splat(value: usize) -> String {
    let lane = format!("i8 {value}");
    format!("<{}>", [lane.as_str(); 16].join(", "))
}
