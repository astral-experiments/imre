define internal i32 @sequence_header(ptr %sequence) alwaysinline nounwind {
  %offset = load i64, ptr @sequence_layout
  %field = getelementptr inbounds i8, ptr %sequence, i64 %offset
  %length = load i32, ptr %field, align 4
  %ordinary = icmp ne i32 %length, -1
  br i1 %ordinary, label %cached, label %deferred, !prof !1
cached:
  ret i32 %length
deferred:
  %resolved = call i32 @imre_seq_resolve_header(ptr %sequence)
  ret i32 %resolved
}
define internal i64 @imre_seq_len_fast(ptr %sequence) alwaysinline nounwind {
  %length = call i32 @sequence_header(ptr %sequence)
  %short = icmp ult i32 %length, -2
  br i1 %short, label %cached, label %large, !prof !1
cached:
  %size = zext i32 %length to i64
  %tagged = or i64 %size, 1
  ret i64 %tagged
large:
  %full = call i64 @imre_seq_len(ptr %sequence)
  ret i64 %full
}
define internal ptr @imre_seq_data_readonly_fast(ptr %sequence) alwaysinline nounwind {
  %header_offset = load i64, ptr @sequence_layout
  %field = getelementptr inbounds i8, ptr %sequence, i64 %header_offset
  %header = load i32, ptr %field, align 4
  %ordinary = icmp ne i32 %header, -1
  %inline_bit = trunc i32 %header to i1
  %inline = and i1 %ordinary, %inline_bit
  br i1 %inline, label %small, label %spilled
small:
  %offset = load i64, ptr getelementptr ({ i64, i64 }, ptr @sequence_layout, i32 0, i32 1)
  %data = getelementptr inbounds i8, ptr %sequence, i64 %offset
  ret ptr %data
spilled:
  %heap = call ptr @imre_seq_data_readonly(ptr %sequence)
  ret ptr %heap
}
define internal i64 @imre_seq_borrow_index_proven_fast(ptr %sequence, i64 %index) alwaysinline nounwind {
  %data = call ptr @imre_seq_data_readonly_fast(ptr %sequence)
  %available = icmp ne ptr %data, null
  br i1 %available, label %read, label %failed, !prof !1
failed:
  ret i64 0
read:
  %slot = getelementptr inbounds i64, ptr %data, i64 %index
  %word = load i64, ptr %slot
  ret i64 %word
}
