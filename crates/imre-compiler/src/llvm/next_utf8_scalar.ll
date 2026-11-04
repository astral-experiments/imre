
; The cursor roots the immutable UTF-8 buffer. Raw bytes and ASCII scalars
; each consume one byte. Only a non-ASCII scalar needs the runtime decoder.
define internal i64 @next_utf8_value(ptr %next, ptr %end, i1 %raw) alwaysinline {
entry:
  %current = load ptr, ptr %next
  %more = icmp ne ptr %current, %end
  br i1 %more, label %read, label %exhausted
read:
  %byte = load i8, ptr %current
  %ascii = icmp sge i8 %byte, 0
  %single = or i1 %ascii, %raw
  br i1 %single, label %one, label %unicode
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
