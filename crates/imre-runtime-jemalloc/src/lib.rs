//! Links the native runtime with jemalloc for Linux executables.

pub use imre_runtime::*;

#[cfg(target_os = "linux")]
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
