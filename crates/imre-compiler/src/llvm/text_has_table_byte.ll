; The rooted string supplies immutable UTF-8 bytes. Non-ASCII table entries are zero.
define internal i1 @text_has_table_byte(ptr %text, ptr %table, ptr %match) alwaysinline {
entry:
  %end_slot = alloca ptr
  %start = call ptr @imre_str_bounds(ptr %text, ptr %end_slot)
  %end = load ptr, ptr %end_slot
  %start_addr = ptrtoint ptr %start to i64
  %end_addr = ptrtoint ptr %end to i64
  %length = sub i64 %end_addr, %start_addr
  %has_matcher = icmp ne ptr %match, null
  %enough = icmp uge i64 %length, 8
  %blocks = and i1 %has_matcher, %enough
  br i1 %blocks, label %width, label %scan
width:
  %full = icmp uge i64 %length, 16
  br i1 %full, label %prepare, label %short
short:
  ; Two in-bounds 8-byte loads cover lengths 8..15, including any overlap.
  %back8 = getelementptr inbounds i8, ptr %end, i64 -8
  %front = load <8 x i8>, ptr %start, align 1
  %back = load <8 x i8>, ptr %back8, align 1
  %both = shufflevector <8 x i8> %front, <8 x i8> %back, <16 x i32> <i32 0, i32 1, i32 2, i32 3, i32 4, i32 5, i32 6, i32 7, i32 8, i32 9, i32 10, i32 11, i32 12, i32 13, i32 14, i32 15>
  %short_hit = call i1 %match(<16 x i8> %both)
  ret i1 %short_hit
prepare:
  %last = getelementptr inbounds i8, ptr %end, i64 -16
  br label %block
block:
  %current = phi ptr [ %start, %prepare ], [ %following, %skip ]
  %bytes = load <16 x i8>, ptr %current, align 1
  %block_hit = call i1 %match(<16 x i8> %bytes)
  br i1 %block_hit, label %present, label %skip
skip:
  ; Clamp the final block to the end of the allocation. A repeated byte cannot change any().
  %after_block = getelementptr inbounds i8, ptr %current, i64 16
  %before_last = icmp ult ptr %after_block, %last
  %following = select i1 %before_last, ptr %after_block, ptr %last
  %done = icmp eq ptr %current, %last
  br i1 %done, label %absent, label %block
scan:
  %next = phi ptr [ %start, %entry ], [ %after, %advance ]
  %more = icmp ne ptr %next, %end
  br i1 %more, label %read, label %absent
read:
  %byte = load i8, ptr %next
  %index = zext i8 %byte to i64
  %slot = getelementptr inbounds i8, ptr %table, i64 %index
  %member = load i8, ptr %slot
  %hit = icmp ne i8 %member, 0
  br i1 %hit, label %present, label %advance
advance:
  %after = getelementptr inbounds i8, ptr %next, i64 1
  br label %scan
present:
  ret i1 true
absent:
  ret i1 false
}
