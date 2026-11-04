; The rooted string supplies immutable UTF-8 bytes. Non-ASCII table entries are zero.
define internal i1 @text_has_table_byte(ptr %text, ptr %table) alwaysinline {
entry:
  %end_slot = alloca ptr
  %start = call ptr @imre_str_bounds(ptr %text, ptr %end_slot)
  %end = load ptr, ptr %end_slot
  br label %scan
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
