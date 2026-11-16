declare i32 @imre_list_try_open_append_window(ptr, i64, ptr)
declare i32 @imre_list_append_open_window(ptr, i64, ptr)
declare i64 @imre_list_tail_open_window(ptr, ptr)
declare void @imre_list_commit_append_window(ptr)

define internal void @finish_list_appends(ptr %ctx) alwaysinline {
entry:
  %owner = load ptr, ptr %ctx
  %open = icmp ne ptr %owner, null
  br i1 %open, label %commit, label %done
commit:
  call void @imre_list_commit_append_window(ptr %ctx)
  store ptr null, ptr %ctx
  br label %done
done:
  ret void
}

define internal i32 @append_in_window(ptr %ctx, ptr %list, i64 %word) alwaysinline {
entry:
  %owner = load ptr, ptr %ctx
  %same = icmp eq ptr %owner, %list
  %small = trunc i64 %word to i1
  %nonnull = icmp ne ptr %owner, null
  %can = and i1 %same, %nonnull
  %open = and i1 %can, %small
  br i1 %open, label %space, label %slow
space:
  %np = getelementptr [3 x ptr], ptr %ctx, i32 0, i32 1
  %ep = getelementptr [3 x ptr], ptr %ctx, i32 0, i32 2
  %n = load ptr, ptr %np
  %e = load ptr, ptr %ep
  %room = icmp ne ptr %n, %e
  br i1 %room, label %fast, label %slow
fast:
  store i64 %word, ptr %n
  %next = getelementptr i64, ptr %n, i64 1
  store ptr %next, ptr %np
  ret i32 1
slow:
  call void @finish_list_appends(ptr %ctx)
  %result = call i32 @imre_list_append_open_window(ptr %list, i64 %word, ptr %ctx)
  ret i32 %result
}

; A window opens only after reading or appending an immediate tail. Its logical tail is
; always an initialized immediate integer, even before the first buffered write.
define internal i64 @get_in_append_window(ptr %ctx, ptr %list) alwaysinline {
entry:
  %owner = load ptr, ptr %ctx
  %same = icmp eq ptr %owner, %list
  %nonnull = icmp ne ptr %owner, null
  %open = and i1 %same, %nonnull
  br i1 %open, label %buffer, label %fallback
buffer:
  %cursor = getelementptr [3 x ptr], ptr %ctx, i32 0, i32 1
  %next = load ptr, ptr %cursor
  %last = getelementptr i64, ptr %next, i64 -1
  %word = load i64, ptr %last
  ret i64 %word
fallback:
  call void @finish_list_appends(ptr %ctx)
  %result = call i64 @imre_list_tail_open_window(ptr %list, ptr %ctx)
  ret i64 %result
}
