; A byte class has one membership value for every non-ASCII UTF-8 byte.
; A maximal run starts exactly when membership changes from false to true.
declare ptr @memchr(ptr, i32, i64)
declare i8 @llvm.vector.reduce.add.v16i8(<16 x i8>)

; Test sixteen bytes against the lower bound and width of one inclusive range.
define internal <16 x i1> @regex_range_lanes(<16 x i8> %bytes, i64 %range) alwaysinline {
  %lower = trunc i64 %range to i8
  %shift = lshr i64 %range, 8
  %width = trunc i64 %shift to i8
  %lo_lane = insertelement <16 x i8> poison, i8 %lower, i32 0
  %hi_lane = insertelement <16 x i8> poison, i8 %width, i32 0
  %lo = shufflevector <16 x i8> %lo_lane, <16 x i8> poison, <16 x i32> zeroinitializer
  %hi = shufflevector <16 x i8> %hi_lane, <16 x i8> poison, <16 x i32> zeroinitializer
  %delta = sub <16 x i8> %bytes, %lo
  %inside = icmp ule <16 x i8> %delta, %hi
  ret <16 x i1> %inside
}

define internal i64 @count_ascii_regex_runs(ptr %text, ptr %class, i32 %singleton, i1 %split, i64 %ranges) {
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
  %a = ptrtoint ptr %from to i64
  %b = ptrtoint ptr %end to i64
  %remaining = sub i64 %b, %a
  %large = icmp uge i64 %remaining, 32
  %simple = icmp sge i64 %ranges, 0
  %can_vectorize = and i1 %large, %simple
  br i1 %can_vectorize, label %prepare, label %tail
prepare:
  %block_length = and i64 %remaining, -16
  %block_end = getelementptr inbounds i8, ptr %from, i64 %block_length
  %state = call {i64, i1} @regex_range_blocks(ptr %from, ptr %block_end, i64 %ranges)
  %block_count = extractvalue {i64, i1} %state, 0
  %with_base = add nuw i64 %base_count, %block_count
  %block_last = extractvalue {i64, i1} %state, 1
  br label %tail
tail:
  %tail_start = phi ptr [ %from, %begin ], [ %block_end, %prepare ]
  %tail_prior = phi i1 [ false, %begin ], [ %block_last, %prepare ]
  %tail_count = phi i64 [ %base_count, %begin ], [ %with_base, %prepare ]
  br label %loop
loop:
  %position = phi ptr [ %tail_start, %tail ], [ %next, %read ]
  %previous = phi i1 [ %tail_prior, %tail ], [ %member, %read ]
  %runs = phi i64 [ %tail_count, %tail ], [ %total, %read ]
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
  %answer = call i64 @regex_run_count_word(i64 %count)
  ret i64 %answer
}

; The large-subject loop is shared by compact classes and kept outside the scalar tail.
define internal {i64, i1} @regex_range_blocks(ptr %from, ptr %block_end, i64 %ranges) noinline {
prepare:
  %second_range = lshr i64 %ranges, 16
  %third_range = lshr i64 %ranges, 32
  br label %blocks
blocks:
  %block = phi ptr [ %from, %prepare ], [ %after, %blocks ]
  %carry = phi i1 [ false, %prepare ], [ %last, %blocks ]
  %sum = phi i64 [ 0, %prepare ], [ %added, %blocks ]
  %bytes = load <16 x i8>, ptr %block, align 1
  %in0 = call <16 x i1> @regex_range_lanes(<16 x i8> %bytes, i64 %ranges)
  %in1 = call <16 x i1> @regex_range_lanes(<16 x i8> %bytes, i64 %second_range)
  %in2 = call <16 x i1> @regex_range_lanes(<16 x i8> %bytes, i64 %third_range)
  %either = or <16 x i1> %in0, %in1
  %members = or <16 x i1> %either, %in2
  ; Carry the previous block's final membership into lane zero.
  %shifted = shufflevector <16 x i1> %members, <16 x i1> poison, <16 x i32> <i32 15, i32 0, i32 1, i32 2, i32 3, i32 4, i32 5, i32 6, i32 7, i32 8, i32 9, i32 10, i32 11, i32 12, i32 13, i32 14>
  %prior = insertelement <16 x i1> %shifted, i1 %carry, i32 0
  %nonmembers = icmp eq <16 x i1> %prior, zeroinitializer
  %starts = and <16 x i1> %members, %nonmembers
  %ones = zext <16 x i1> %starts to <16 x i8>
  %pop = call i8 @llvm.vector.reduce.add.v16i8(<16 x i8> %ones)
  %n = zext i8 %pop to i64
  %added = add nuw i64 %sum, %n
  %last = extractelement <16 x i1> %members, i32 15
  %after = getelementptr inbounds i8, ptr %block, i64 16
  %at_end = icmp eq ptr %after, %block_end
  br i1 %at_end, label %finish, label %blocks
finish:
  %sum_result = insertvalue {i64, i1} poison, i64 %added, 0
  %result = insertvalue {i64, i1} %sum_result, i1 %last, 1
  ret {i64, i1} %result
}
