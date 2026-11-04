
; The cursor roots a valid UTF-8 buffer. The compiler owns this private byte
; position, so a successful ASCII pull can advance it without a runtime call.
define internal i64 @next_utf8_scalar(ptr %next, ptr %end) alwaysinline {
entry:
  %current = load ptr, ptr %next
  %more = icmp ne ptr %current, %end
  br i1 %more, label %read, label %exhausted
read:
  %byte = load i8, ptr %current
  %ascii = icmp sge i8 %byte, 0
  br i1 %ascii, label %one, label %unicode
one:
  %after = getelementptr inbounds i8, ptr %current, i64 1
  store ptr %after, ptr %next
  %scalar = zext i8 %byte to i64
  %payload = shl nuw nsw i64 %scalar, 1
  %tagged = or i64 %payload, 1
  ret i64 %tagged
unicode:
  %wide = call i64 @imre_utf8_next_non_ascii(ptr %next, ptr %end)
  ret i64 %wide
exhausted:
  ret i64 0
}
