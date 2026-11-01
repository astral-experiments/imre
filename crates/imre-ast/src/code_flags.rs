//! CPython 3.14 `code.co_flags` values shared by the compiler and boxed runtime.
pub const OPTIMIZED: u32 = 1;
pub const NEWLOCALS: u32 = 2;
pub const VARARGS: u32 = 4;
pub const VARKEYWORDS: u32 = 8;
pub const NESTED: u32 = 0x10;
pub const GENERATOR: u32 = 0x20;
pub const COROUTINE: u32 = 0x80;
pub const ITERABLE_COROUTINE: u32 = 0x100;
pub const ASYNC_GENERATOR: u32 = 0x200;
pub const FUTURE_BARRY_AS_BDFL: u32 = 0x0040_0000;
pub const FUTURE_ANNOTATIONS: u32 = 0x0100_0000;
pub const HAS_DOCSTRING: u32 = 0x0400_0000;
pub const METHOD: u32 = 0x0800_0000;
