; A byte class has one membership value for every non-ASCII UTF-8 byte.
; A maximal run starts exactly when membership changes from false to true.
declare ptr @memchr(ptr, i32, i64)

define internal i64 @count_ascii_regex_runs(ptr %text, ptr %class, i32 %singleton, i1 %split) {
entry:
  %end_slot = alloca ptr
  %start = call ptr @imre_str_bounds(ptr %text, ptr %end_slot)
  %end = load ptr, ptr %end_slot
  %base_count = zext i1 %split to i64
  %single = icmp sge i32 %singleton, 0
  br i1 %single, label %find_first, label %begin
find_first:
  %end_word = ptrtoint ptr %end to i64
  %start_word = ptrtoint ptr %start to i64
  %length = sub i64 %end_word, %start_word
  %first = call ptr @memchr(ptr %start, i32 %singleton, i64 %length)
  %absent = icmp eq ptr %first, null
  br i1 %absent, label %finish, label %begin
begin:
  %from = phi ptr [ %start, %entry ], [ %first, %find_first ]
  br label %loop
loop:
  %position = phi ptr [ %from, %begin ], [ %next, %read ]
  %previous = phi i1 [ false, %begin ], [ %member, %read ]
  %runs = phi i64 [ %base_count, %begin ], [ %total, %read ]
  %done = icmp eq ptr %position, %end
  br i1 %done, label %finish, label %read
read:
  %byte = load i8, ptr %position, align 1
  %index = zext i8 %byte to i64
  %slot = getelementptr inbounds i8, ptr %class, i64 %index
  %flag = load i8, ptr %slot, align 1
  %member = icmp ne i8 %flag, 0
  %outside = xor i1 %previous, true
  %enter = and i1 %member, %outside
  %increment = zext i1 %enter to i64
  %total = add nuw i64 %runs, %increment
  %next = getelementptr inbounds i8, ptr %position, i64 1
  br label %loop
finish:
  %count = phi i64 [ %base_count, %find_first ], [ %runs, %loop ]
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
