
; Count false-to-true transitions while advancing through complete Unicode scalars.
declare i32 @llvm.vector.reduce.or.v4i32(<4 x i32>)

define internal i64 @count_unicode_regex_runs(ptr %text, ptr %class, i64 %size, i1 %split) {
entry:
  %end_slot = alloca ptr
  %cursor = alloca ptr
  %start = call ptr @imre_str_bounds(ptr %text, ptr %end_slot)
  %end = load ptr, ptr %end_slot
  %bounds = getelementptr inbounds i8, ptr %class, i64 256
  %initial = zext i1 %split to i64
  br label %loop
loop:
  %position = phi ptr [ %start, %entry ], [ %next, %consume ]
  %runs = phi i64 [ %initial, %entry ], [ %total, %consume ]
  %previous = phi i1 [ false, %entry ], [ %member, %consume ]
  %done = icmp eq ptr %position, %end
  br i1 %done, label %finish, label %read
read:
  %byte = load i8, ptr %position, align 1
  %ascii = icmp sge i8 %byte, 0
  br i1 %ascii, label %lookup, label %decode
lookup:
  %index = zext i8 %byte to i64
  %slot = getelementptr inbounds i8, ptr %class, i64 %index
  %flag = load i8, ptr %slot, align 1
  %matched_byte = icmp ne i8 %flag, 0
  %after_byte = getelementptr inbounds i8, ptr %position, i64 1
  br label %consume
decode:
  store ptr %position, ptr %cursor
  %tagged = call i64 @imre_utf8_next_non_ascii(ptr %cursor, ptr %end)
  %value = lshr i64 %tagged, 1
  %scalar = trunc i64 %value to i32
  %after_scalar = load ptr, ptr %cursor
  br label %search
search:
  ; Find the number of ordered range boundaries at or below this scalar.
  %low = phi i64 [ 0, %decode ], [ %lower, %probe ]
  %high = phi i64 [ %size, %decode ], [ %upper, %probe ]
  %found = icmp eq i64 %low, %high
  br i1 %found, label %parity, label %probe
probe:
  %length = sub nuw i64 %high, %low
  %half = lshr i64 %length, 1
  %mid = add nuw i64 %low, %half
  %address = getelementptr inbounds [4 x i8], ptr %bounds, i64 %mid
  ; Range boundaries are serialized little-endian, including on big-endian hosts.
  %bytes = load <4 x i8>, ptr %address, align 1
  %lanes = zext <4 x i8> %bytes to <4 x i32>
  %shifted = shl <4 x i32> %lanes, <i32 0, i32 8, i32 16, i32 24>
  %bound = call i32 @llvm.vector.reduce.or.v4i32(<4 x i32> %shifted)
  %before = icmp ule i32 %bound, %scalar
  %past = add nuw i64 %mid, 1
  %lower = select i1 %before, i64 %past, i64 %low
  %upper = select i1 %before, i64 %high, i64 %mid
  br label %search
parity:
  %matched_scalar = trunc i64 %low to i1
  br label %consume
consume:
  %member = phi i1 [ %matched_byte, %lookup ], [ %matched_scalar, %parity ]
  %next = phi ptr [ %after_byte, %lookup ], [ %after_scalar, %parity ]
  %outside = xor i1 %previous, true
  %enter = and i1 %member, %outside
  %increment = zext i1 %enter to i64
  %total = add nuw i64 %runs, %increment
  br label %loop
finish:
  %answer = call i64 @regex_run_count_word(i64 %runs)
  ret i64 %answer
}
