; Visit the flat bitmap in modular index order. The dictionary view has at least
; one allocated word, and omitted trailing words represent default rows.
define internal { double, i64 } @imre_bitmap_indexed_sum(ptr %view, i64 %start, i64 %stop, i64 %modulus, i64 %scale, i64 %offset, double %initial, double %zero, double %one) nounwind {
entry:
  %bits_ptr = getelementptr ptr, ptr %view, i64 2
  %words_ptr = getelementptr i64, ptr %view, i64 3
  %bits = load ptr, ptr %bits_ptr
  %words = load i64, ptr %words_ptr
  %n = lshr i64 %modulus, 1
  %step = urem i64 %scale, %n
  %first_tag = call i64 @imre_bounded_modular_index(i64 %start, i64 %scale, i64 %offset, i64 %modulus)
  %first = lshr i64 %first_tag, 1
  br label %poll
poll:
  %cursor = phi i64 [ %start, %entry ], [ %end, %inner ]
  %at = phi i64 [ %first, %entry ], [ %next, %inner ]
  %sum = phi double [ %initial, %entry ], [ %added, %inner ]
  %signal = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %pending = icmp ne i8 %signal, 0
  %done = icmp eq i64 %cursor, %stop
  %leave = or i1 %pending, %done
  br i1 %leave, label %exit, label %chunk
chunk:
  %left = sub i64 %stop, %cursor
  %short = icmp ult i64 %left, 2046
  %width = select i1 %short, i64 %left, i64 2046
  %end = add i64 %cursor, %width
  br label %inner
inner:
  %ordinal = phi i64 [ %cursor, %chunk ], [ %advance, %inner ]
  %pos = phi i64 [ %at, %chunk ], [ %next, %inner ]
  %subtotal = phi double [ %sum, %chunk ], [ %added, %inner ]
  %word = lshr i64 %pos, 6
  %stored = icmp ult i64 %word, %words
  %safe = select i1 %stored, i64 %word, i64 0
  %slot = getelementptr inbounds i64, ptr %bits, i64 %safe
  %tags = load i64, ptr %slot
  %shift = and i64 %pos, 63
  %tag = lshr i64 %tags, %shift
  %alternate = trunc i64 %tag to i1
  %choose = and i1 %stored, %alternate
  %term = select i1 %choose, double %one, double %zero
  %added = fadd double %subtotal, %term
  %raw = add i64 %pos, %step
  %wrap = icmp uge i64 %raw, %n
  %wrapped = sub i64 %raw, %n
  %next = select i1 %wrap, i64 %wrapped, i64 %raw
  %advance = add i64 %ordinal, 2
  %last = icmp eq i64 %advance, %end
  br i1 %last, label %poll, label %inner
exit:
  %value = insertvalue { double, i64 } poison, double %sum, 0
  %result = insertvalue { double, i64 } %value, i64 %cursor, 1
  ret { double, i64 } %result
}
