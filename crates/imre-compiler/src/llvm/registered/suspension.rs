//! LLVM splits each suspended body into its entry, resume and destroy functions.
//! Its heap frame contains the live machine values. Python handles remain rooted by
//! the ordinary activation and its delegate, using the same statement checkpoints.

/// The runtime owns the sized, aligned frame storage through the final destroy.
pub(crate) const ABI: &str = r#"
declare token @llvm.coro.id(i32, ptr, ptr, ptr)
declare i64 @llvm.coro.size.i64()
declare i32 @llvm.coro.align.i32()
declare ptr @llvm.coro.begin(token, ptr)
declare token @llvm.coro.save(ptr)
declare i8 @llvm.coro.suspend(token, i1)
declare i1 @llvm.coro.end(ptr, i1, token)
declare void @llvm.coro.resume(ptr)
declare void @llvm.coro.destroy(ptr)
declare noalias ptr @imre_dyn_coro_alloc(ptr, i64, i32)
declare void @imre_dyn_coro_open(ptr, ptr, ptr)
declare void @imre_dyn_suspend_operands(ptr, i64, i64, i32)
declare i64 @imre_dyn_resume_input(ptr)
declare void @imre_dyn_suspend_return(ptr, i64)

define internal void @imre_boxed_drive(ptr %handle, i1 zeroext %destroy) {
entry:
  br i1 %destroy, label %release, label %resume
resume:
  call void @llvm.coro.resume(ptr %handle)
  ret void
release:
  call void @llvm.coro.destroy(ptr %handle)
  ret void
}
"#;

pub(crate) const PROLOGUE: &str = r#"  %coro_id = call token @llvm.coro.id(i32 0, ptr null, ptr null, ptr null)
  %frame_bytes = call i64 @llvm.coro.size.i64()
  %frame_align = call i32 @llvm.coro.align.i32()
  %storage = call ptr @imre_dyn_coro_alloc(ptr %context_slot, i64 %frame_bytes, i32 %frame_align)
  %allocated = icmp ne ptr %storage, null
  br i1 %allocated, label %start, label %no_memory
no_memory:
  ret ptr null
start:
  %handle = call ptr @llvm.coro.begin(token %coro_id, ptr %storage)
  call void @imre_dyn_coro_open(ptr %context_slot, ptr %handle, ptr @imre_boxed_drive)
"#;

pub(crate) const EPILOGUE: &str = r#"complete:
  %final_save = call token @llvm.coro.save(ptr %handle)
  %final = call i8 @llvm.coro.suspend(token %final_save, i1 true)
  switch i8 %final, label %parked [ i8 0, label %invalid_resume
                                    i8 1, label %discard ]
invalid_resume:
  unreachable
discard:
  br label %parked
parked:
  %end = call i1 @llvm.coro.end(ptr %handle, i1 false, token none)
  ret ptr %handle
"#;

/// Operation numbers in the boxed suspension ABI.
#[repr(i32)]
pub(crate) enum Suspension {
    Yield = 0,
    AsyncYield = 1,
    Await = 2,
    Delegate = 3,
}
