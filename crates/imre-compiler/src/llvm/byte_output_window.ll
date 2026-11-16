declare void @imre_byte_output_reserve_window(ptr, i64, i64, i64, i64, ptr)
declare i32 @imre_byte_output_grow_window(ptr, i64, i64, i64, ptr)
declare ptr @imre_byte_output_finish_window(ptr, ptr)

; The caller owns the private buffer. Its scoped alias shares this cursor.
; Count is a compile-time constant of one or two. A single append passes the
; tagged zero as its second word so that it contributes no invalid bits.
define internal i32 @append_private_bytes(ptr %ctx, ptr %output, i64 %a, i64 %b, i64 %count) alwaysinline {
entry:
  %room_ptr = getelementptr { ptr, i64, i64 }, ptr %ctx, i32 0, i32 1
  %bits_ptr = getelementptr { ptr, i64, i64 }, ptr %ctx, i32 0, i32 2
  %room = load i64, ptr %room_ptr
  %fits = icmp uge i64 %room, %count
  %pair = icmp eq i64 %count, 2
  br i1 %fits, label %write, label %grow
write:
  %dst = load ptr, ptr %ctx
  %first_word = lshr i64 %a, 1
  %first = trunc i64 %first_word to i8
  store i8 %first, ptr %dst
  br i1 %pair, label %second, label %advance
second:
  %second_dst = getelementptr inbounds i8, ptr %dst, i64 1
  %second_word = lshr i64 %b, 1
  %last = trunc i64 %second_word to i8
  store i8 %last, ptr %second_dst
  br label %advance
advance:
  %old = load i64, ptr %bits_ptr
  %a_bits = xor i64 %a, 1
  %b_bits = xor i64 %b, 1
  %added = or i64 %a_bits, %b_bits
  %bits = or i64 %old, %added
  store i64 %bits, ptr %bits_ptr
  %next = getelementptr inbounds i8, ptr %dst, i64 %count
  store ptr %next, ptr %ctx
  %remaining = sub nuw i64 %room, %count
  store i64 %remaining, ptr %room_ptr
  ret i32 1
grow:
  %status = call i32 @imre_byte_output_grow_window(ptr %output, i64 %a, i64 %b, i64 %count, ptr %ctx)
  ret i32 %status
}
