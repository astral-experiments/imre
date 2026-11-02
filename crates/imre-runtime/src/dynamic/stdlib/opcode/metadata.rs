// Generated from CPython 5607950ef232dad16d75c0cf53101d9649d89115 by scripts/generate_opcode_metadata.py.
// CPython license: licenses/CPython.txt.

// Bits 0–5 represent the six HAS_* flags, bit 6 block pushes, and bit 7 valid entries.
#[rustfmt::skip]
const FLAGS: [u8; 267] = [
    128, 128, 128, 160, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 129, 129, 129, 129,
    129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 133, 145, 161,
    133, 133, 129, 129, 137, 129, 137, 129, 133, 133, 129, 137, 137, 137, 129, 129,
    133, 129, 131, 161, 161, 161, 161, 161, 161, 161, 145, 133, 133, 133, 129, 129,
    133, 145, 129, 129, 137, 137, 137, 137, 129, 129, 137, 129, 129, 129, 133, 145,
    161, 161, 161, 133, 133, 129, 129, 129, 129,   0,   0,   0,   0,   0,   0,   0,
    129, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 129,
    129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 128, 129, 129, 129, 129, 129,
    129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 129, 137, 137, 137, 137,
    137, 129, 129, 133, 129, 129, 129, 129, 129, 129, 129, 129, 129, 133, 131, 131,
    129, 129, 133, 133, 128, 129, 128, 128, 133, 128, 128, 128, 128, 128, 128, 128,
    128, 129, 129, 129,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
      0,   0,   0,   0,   0,   0,   0,   0,   0,   0, 128, 128, 128, 137, 128, 129,
    128, 129, 129, 129, 129, 129, 128, 129, 137, 133, 129, 129, 128, 129, 128, 129,
    128, 137, 137, 137, 137, 161, 128, 193, 193, 193, 161,
];

pub(super) fn predicate(name: &str, opcode: i32) -> bool {
    let mask = match name {
        "has_arg" => 1,
        "has_const" => 2,
        "has_name" => 4,
        "has_jump" => 8,
        "has_free" => 16,
        "has_local" => 32,
        "has_exc" => 64,
        "is_valid" => 128,
        _ => return false,
    };
    FLAGS
        .get(opcode as usize)
        .is_some_and(|flags| flags & mask != 0)
}

pub(super) fn stack_effect(opcode: i32, oparg: i32, jump: i32) -> Option<i32> {
    // CPython flowgraph.c:get_stack_effects accepts base and pseudo instructions.
    // Evaluate the C stack-count expressions with wrapping signed 32-bit results.
    let oparg = i64::from(oparg);
    let (popped, pushed): (i64, i64) = match opcode {
        0
        | 17
        | 27..=28
        | 36
        | 60
        | 62..=65
        | 69
        | 75..=77
        | 97
        | 128
        | 238..=240
        | 245
        | 253..=257
        | 260
        | 262 => (0, 0),
        1 | 99 => (3, 1),
        2 | 10 | 13 | 44 | 54 | 56..=57 | 73..=74 | 108 | 234 | 236 => (2, 1),
        4 | 252 => (4, 1),
        5..=6 | 106 => (2, 2),
        7 => (3, 2),
        8 | 68 | 110 | 114 | 248 => (2, 0),
        9 | 11 | 20 | 29..=31 | 61 | 100..=103 | 111..=112 | 115..=116 | 235 | 241..=244 | 266 => {
            (1, 0)
        }
        12
        | 14
        | 16
        | 19
        | 23
        | 35
        | 39..=42
        | 53
        | 58
        | 71
        | 90..=91
        | 113
        | 120
        | 246..=247
        | 258..=259 => (1, 1),
        15 | 18 | 25..=26 | 32 | 70 | 72 | 95 | 237 => (1, 2),
        21..=22 | 33..=34 | 81..=86 | 88 | 93..=94 | 261 | 264..=265 => (0, 1),
        24 => (2, 3),
        37 => (4, 0),
        38 => (3, 0),
        43 => (5, 6),
        45 => (2 + (oparg & 1), 1),
        46 | 48..=51 => (oparg, 1),
        47 => (oparg * 2, 1),
        52 | 250 => (2 + oparg, 1),
        55 | 251 => (3 + oparg, 1),
        59 => (1 + (oparg - 1), 2 + (oparg - 1)),
        66 => (5 + (oparg - 1), 4 + (oparg - 1)),
        67 => (2 + (oparg - 1), 1 + (oparg - 1)),
        78..=79 | 107 | 109 => (2 + (oparg - 1), 1 + (oparg - 1)),
        80 => (1, 1 + (oparg & 1)),
        87 | 89 | 263 => (0, 2),
        92 => (0, 1 + (oparg & 1)),
        96 | 249 => (3, 1 + (oparg & 1)),
        98 => (3 + (oparg - 1), 1 + (oparg - 1)),
        104 => (oparg, 0),
        105 => (1 + oparg, oparg),
        117 => (2 + (oparg - 2), 2 + (oparg - 2)),
        118 => (1, 1 + (oparg & 0xFF) + (oparg >> 8)),
        119 => (1, oparg),
        _ => return None,
    };
    let (popped, pushed) = (popped as i32, pushed as i32);
    if popped < 0 || pushed < 0 {
        return None;
    }
    if jump == 0 && matches!(opcode, 263..=265) {
        return Some(0);
    }
    let effect = pushed - popped;
    (effect != i32::MAX).then_some(effect)
}

pub(super) const NB_OPS: &[(&str, &str)] = &[
    ("NB_ADD", "+"),
    ("NB_AND", "&"),
    ("NB_FLOOR_DIVIDE", "//"),
    ("NB_LSHIFT", "<<"),
    ("NB_MATRIX_MULTIPLY", "@"),
    ("NB_MULTIPLY", "*"),
    ("NB_REMAINDER", "%"),
    ("NB_OR", "|"),
    ("NB_POWER", "**"),
    ("NB_RSHIFT", ">>"),
    ("NB_SUBTRACT", "-"),
    ("NB_TRUE_DIVIDE", "/"),
    ("NB_XOR", "^"),
    ("NB_INPLACE_ADD", "+="),
    ("NB_INPLACE_AND", "&="),
    ("NB_INPLACE_FLOOR_DIVIDE", "//="),
    ("NB_INPLACE_LSHIFT", "<<="),
    ("NB_INPLACE_MATRIX_MULTIPLY", "@="),
    ("NB_INPLACE_MULTIPLY", "*="),
    ("NB_INPLACE_REMAINDER", "%="),
    ("NB_INPLACE_OR", "|="),
    ("NB_INPLACE_POWER", "**="),
    ("NB_INPLACE_RSHIFT", ">>="),
    ("NB_INPLACE_SUBTRACT", "-="),
    ("NB_INPLACE_TRUE_DIVIDE", "/="),
    ("NB_INPLACE_XOR", "^="),
    ("NB_SUBSCR", "[]"),
];

pub(super) const INTRINSIC1: &[&str] = &[
    "INTRINSIC_1_INVALID",
    "INTRINSIC_PRINT",
    "INTRINSIC_IMPORT_STAR",
    "INTRINSIC_STOPITERATION_ERROR",
    "INTRINSIC_ASYNC_GEN_WRAP",
    "INTRINSIC_UNARY_POSITIVE",
    "INTRINSIC_LIST_TO_TUPLE",
    "INTRINSIC_TYPEVAR",
    "INTRINSIC_PARAMSPEC",
    "INTRINSIC_TYPEVARTUPLE",
    "INTRINSIC_SUBSCRIPT_GENERIC",
    "INTRINSIC_TYPEALIAS",
];

pub(super) const INTRINSIC2: &[&str] = &[
    "INTRINSIC_2_INVALID",
    "INTRINSIC_PREP_RERAISE_STAR",
    "INTRINSIC_TYPEVAR_WITH_BOUND",
    "INTRINSIC_TYPEVAR_WITH_CONSTRAINTS",
    "INTRINSIC_SET_FUNCTION_TYPE_PARAMS",
    "INTRINSIC_SET_TYPEPARAM_DEFAULT",
];

pub(super) const SPECIAL_METHODS: &[&str] = &["__enter__", "__exit__", "__aenter__", "__aexit__"];
