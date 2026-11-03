; Visit the flat bitmap in modular index order. The dictionary view has at least
; one allocated word, and omitted trailing words represent default rows.
define internal { double, i64 } @imre_bitmap_indexed_sum(ptr %view, i64 %start, i64 %stop, i64 %modulus, i64 %tagged_length, i64 %prefix_ones, i64 %scale, i64 %offset, double %initial, double %zero, double %one) nounwind {
entry:
  %n = lshr i64 %modulus, 1
  %step = urem i64 %scale, %n
  %first_tag = call i64 @imre_bounded_modular_index(i64 %start, i64 %scale, i64 %offset, i64 %modulus)
  %first = lshr i64 %first_tag, 1
  %full_view = icmp eq i64 %tagged_length, %modulus
  ; A modular index is nonnegative, so -1 marks a view that omits rows.
  %origin = select i1 %full_view, i64 %first, i64 -1
  %cycles = call { double, i64 } @imre_bitmap_permutation_prefix(ptr %view, i64 %start, i64 %stop, i64 %n, i64 %origin, i64 %step, i64 %prefix_ones, double %initial, double %zero, double %one)
  %base_sum = extractvalue { double, i64 } %cycles, 0
  %base_cursor = extractvalue { double, i64 } %cycles, 1
  %bits_ptr = getelementptr ptr, ptr %view, i64 2
  %words_ptr = getelementptr i64, ptr %view, i64 3
  %bits = load ptr, ptr %bits_ptr
  %words = load i64, ptr %words_ptr
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
; A long partial period can subtract the omitted tags from one complete period.
; Both folds require an exact sum for every possible prefix.
; Outline the proof so the indexed scan stays small enough to inline.
define internal { double, i64 } @imre_bitmap_permutation_prefix(ptr %view, i64 %start, i64 %stop, i64 %n, i64 %first, i64 %step, i64 %prefix_ones, double %initial, double %zero, double %one) noinline nounwind {
entry:
  %distance = sub i64 %stop, %start
  %remaining = lshr i64 %distance, 1
  ; Counting a tag also checks for interrupts. Require at most one omitted
  ; row per two visited rows before choosing the complement.
  %third = udiv i64 %n, 3
  %threshold = sub i64 %n, %third
  %enough = icmp uge i64 %remaining, %threshold
  %full_view = icmp sge i64 %first, 0
  %candidate = and i1 %full_view, %enough
  br i1 %candidate, label %orbit, label %miss
orbit:
  %proof = call { i64, i64 } @imre_modular_bezout(i64 %step, i64 %n)
  %gcd = extractvalue { i64, i64 } %proof, 0
  %permutation = icmp eq i64 %gcd, 1
  %periods = udiv i64 %remaining, %n
  %whole = mul i64 %periods, %n
  %partial = sub i64 %remaining, %whole
  %omitted = sub i64 %n, %partial
  %shorter = icmp uge i64 %partial, %threshold
  %visits = select i1 %shorter, i64 %remaining, i64 %whole
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
  %bits_ptr = getelementptr ptr, ptr %view, i64 2
  %words_ptr = getelementptr i64, ptr %view, i64 3
  %bits = load ptr, ptr %bits_ptr
  %words = load i64, ptr %words_ptr
  ; The existing prefix count excludes the physical last row.
  %last = sub i64 %n, 1
  %last_tag = call i1 @imre_bitmap_tag(ptr %bits, i64 %words, i64 %last)
  %extra = zext i1 %last_tag to i64
  %per_period = add i64 %prefix_ones, %extra
  %full_ones = mul i64 %periods, %per_period
  br i1 %shorter, label %complement, label %counts
complement:
  ; Walk backwards from the start of the next period to count the omitted rows.
  %absent = call i64 @imre_bitmap_omitted_ones(ptr %bits, i64 %words, i64 %n, i64 %first, i64 %step, i64 %omitted)
  %interrupted = icmp slt i64 %absent, 0
  %partial_ones = sub i64 %per_period, %absent
  %included = add i64 %full_ones, %partial_ones
  br i1 %interrupted, label %miss, label %counts
counts:
  %ones = phi i64 [ %full_ones, %reduce ], [ %included, %complement ]
  %zeros = sub i64 %visits, %ones
  %nf_zero = uitofp i64 %zeros to double
  %nf_one = uitofp i64 %ones to double
  %sum_zero = fmul double %zero, %nf_zero
  %sum_one = fmul double %one, %nf_one
  ; An absent row must not turn a negative zero into a positive zero.
  %has_zero = icmp ne i64 %zeros, 0
  %has_one = icmp ne i64 %ones, 0
  %with_zero = fadd double %initial, %sum_zero
  %subtotal = select i1 %has_zero, double %with_zero, double %initial
  %with_one = fadd double %subtotal, %sum_one
  %total = select i1 %has_one, double %with_one, double %subtotal
  %width = shl i64 %visits, 1
  %end = add i64 %start, %width
  br label %done
miss:
  br label %done
done:
  %sum = phi double [ %initial, %miss ], [ %total, %counts ]
  %cursor = phi i64 [ %start, %miss ], [ %end, %counts ]
  %value = insertvalue { double, i64 } poison, double %sum, 0
  %result = insertvalue { double, i64 } %value, i64 %cursor, 1
  ret { double, i64 } %result
}

; Count tags in the shorter, unvisited suffix. A pending interrupt discards the
; speculative count before the caller publishes any loop state.
define internal i64 @imre_bitmap_omitted_ones(ptr %bits, i64 %words, i64 %n, i64 %first, i64 %step, i64 %omitted) nounwind {
entry:
  br label %loop
loop:
  %at = phi i64 [ %first, %entry ], [ %prev, %visit ]
  %left = phi i64 [ %omitted, %entry ], [ %rest, %visit ]
  %count = phi i64 [ 0, %entry ], [ %added, %visit ]
  %signal = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %pending = icmp ne i8 %signal, 0
  br i1 %pending, label %interrupt, label %visit
visit:
  %raw = sub i64 %at, %step
  %wrap = icmp ult i64 %at, %step
  %wrapped = add i64 %raw, %n
  %prev = select i1 %wrap, i64 %wrapped, i64 %raw
  %bit = call i1 @imre_bitmap_tag(ptr %bits, i64 %words, i64 %prev)
  %one = zext i1 %bit to i64
  %added = add i64 %count, %one
  %rest = sub i64 %left, 1
  %done = icmp eq i64 %rest, 0
  br i1 %done, label %exit, label %loop
interrupt:
  ret i64 -1
exit:
  ret i64 %added
}
