
; Encode a Python length after either byte or Unicode class counting.
define internal i64 @regex_run_count_word(i64 %count) alwaysinline {
entry:
  %small = icmp ule i64 %count, 4611686018427387903
  br i1 %small, label %tag, label %wide
tag:
  %bits = shl i64 %count, 1
  %word = or i64 %bits, 1
  ret i64 %word
wide:
  ; At most ceil(length / 2) runs plus one split field fit in an addressable string.
  %extra = sub nuw i64 %count, 4611686018427387903
  %shift = shl nuw i64 %extra, 1
  %right = or i64 %shift, 1
  %big = call i64 @imre_int_binary(i64 9223372036854775807, i64 %right, i32 0)
  ret i64 %big
}
