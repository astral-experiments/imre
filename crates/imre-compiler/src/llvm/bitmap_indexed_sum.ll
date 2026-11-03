; Visit the flat bitmap in modular index order. The dictionary view has at least
; one allocated word, and omitted trailing words represent default rows.
define internal { double, i64 } @imre_bitmap_indexed_sum(ptr %view, i64 %start, i64 %stop, i64 %modulus, i64 %tagged_length, i64 %prefix_ones, i64 %scale, i64 %offset, double %initial, double %zero, double %one) nounwind {
entry:
  %bits_ptr = getelementptr ptr, ptr %view, i64 2
  %words_ptr = getelementptr i64, ptr %view, i64 3
  %bits = load ptr, ptr %bits_ptr
  %words = load i64, ptr %words_ptr
  %n = lshr i64 %modulus, 1
  %step = urem i64 %scale, %n
  %first_tag = call i64 @imre_bounded_modular_index(i64 %start, i64 %scale, i64 %offset, i64 %modulus)
  %first = lshr i64 %first_tag, 1
  %full_view = icmp eq i64 %tagged_length, %modulus
  %cycles = call { double, i64 } @imre_bitmap_complete_cycles(ptr %bits, i64 %words, i64 %start, i64 %stop, i64 %n, i1 %full_view, i64 %step, i64 %prefix_ones, double %initial, double %zero, double %one)
  %base_sum = extractvalue { double, i64 } %cycles, 0
  %base_cursor = extractvalue { double, i64 } %cycles, 1
  br label %poll
poll:
  %cursor = phi i64 [ %base_cursor, %entry ], [ %end, %inner ]
  %at = phi i64 [ %first, %entry ], [ %next, %inner ]
  %sum = phi double [ %base_sum, %entry ], [ %added, %inner ]
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
  %choose = call i1 @imre_bitmap_tag(ptr %bits, i64 %words, i64 %pos)
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

; A successful binary view contains at least one stored word. Trailing words are zero.
define internal i1 @imre_bitmap_tag(ptr %bits, i64 %words, i64 %pos) alwaysinline nounwind {
  %word = lshr i64 %pos, 6
  %stored = icmp ult i64 %word, %words
  %safe = select i1 %stored, i64 %word, i64 0
  %slot = getelementptr inbounds i64, ptr %bits, i64 %safe
  %tags = load i64, ptr %slot
  %shift = and i64 %pos, 63
  %tag = lshr i64 %tags, %shift
  %alternate = trunc i64 %tag to i1
  %choose = and i1 %stored, %alternate
  ret i1 %choose
}

; A coprime modular step permutes the whole list in each complete period.
; Use the maintained dictionary count only when every possible partial sum is exact.
; Outline the proof so the indexed scan stays small enough to inline.
define internal { double, i64 } @imre_bitmap_complete_cycles(ptr %bits, i64 %words, i64 %start, i64 %stop, i64 %n, i1 %full_view, i64 %step, i64 %prefix_ones, double %initial, double %zero, double %one) noinline nounwind {
entry:
  %distance = sub i64 %stop, %start
  %remaining = lshr i64 %distance, 1
  %enough = icmp uge i64 %remaining, %n
  %candidate = and i1 %full_view, %enough
  br i1 %candidate, label %orbit, label %miss
orbit:
  %proof = call { i64, i64 } @imre_modular_bezout(i64 %step, i64 %n)
  %gcd = extractvalue { i64, i64 } %proof, 0
  %permutation = icmp eq i64 %gcd, 1
  %periods = udiv i64 %remaining, %n
  %visits = mul i64 %periods, %n
  %exact_zero = call i1 @imre_exact_fadd_bound(double %initial, double %zero, i64 %visits)
  %exact_one = call i1 @imre_exact_fadd_bound(double %initial, double %one, i64 %visits)
  %exact = and i1 %exact_zero, %exact_one
  %eligible = and i1 %exact, %permutation
  br i1 %eligible, label %interrupt, label %miss
interrupt:
  %signal = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %clear = icmp eq i8 %signal, 0
  br i1 %clear, label %reduce, label %miss
reduce:
  ; The existing prefix count excludes the physical last row.
  %last = sub i64 %n, 1
  %last_tag = call i1 @imre_bitmap_tag(ptr %bits, i64 %words, i64 %last)
  %extra = zext i1 %last_tag to i64
  %per_period = add i64 %prefix_ones, %extra
  %ones = mul i64 %periods, %per_period
  %zeros = sub i64 %visits, %ones
  %nf_zero = uitofp i64 %zeros to double
  %nf_one = uitofp i64 %ones to double
  %sum_zero = fmul double %zero, %nf_zero
  %sum_one = fmul double %one, %nf_one
  %subtotal = fadd double %initial, %sum_zero
  %total = fadd double %subtotal, %sum_one
  %width = shl i64 %visits, 1
  %end = add i64 %start, %width
  br label %done
miss:
  br label %done
done:
  %sum = phi double [ %initial, %miss ], [ %total, %reduce ]
  %cursor = phi i64 [ %start, %miss ], [ %end, %reduce ]
  %value = insertvalue { double, i64 } poison, double %sum, 0
  %result = insertvalue { double, i64 } %value, i64 %cursor, 1
  ret { double, i64 } %result
}
