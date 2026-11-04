; Sum two projected rows, using precomputed visit counts or a flat tag bitmap.
; Missing trailing bitmap words represent default rows.
define internal { double, i64 } @imre_bitmap_indexed_sum(ptr %view, i64 %start, i64 %stop, i64 %modulus, i64 %tagged_length, i64 %prefix_ones, i64 %scale, i64 %offset, double %initial, double %zero, double %one) noinline nounwind {
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
  %precounted = icmp eq ptr %bits, null
  br i1 %precounted, label %precounted_result, label %poll
precounted_result:
  ret { double, i64 } %cycles
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
; Keep one-time orbit planning compact and separate from the ordered traversal.
define internal { double, i64 } @imre_bitmap_permutation_prefix(ptr %view, i64 %start, i64 %stop, i64 %n, i64 %first, i64 %step, i64 %prefix_ones, double %initial, double %zero, double %one) noinline optsize nounwind {
entry:
  %distance = sub i64 %stop, %start
  %remaining = lshr i64 %distance, 1
  %bits_ptr = getelementptr ptr, ptr %view, i64 2
  %bits = load ptr, ptr %bits_ptr
  %precounted = icmp eq ptr %bits, null
  br i1 %precounted, label %known_counts, label %plan
known_counts:
  %period = phi i64 [ 0, %entry ], [ %orbit_length, %repeated ]
  ; Both paths know the number of visits and require an exact sum.
  %safe_zero = call i1 @imre_exact_fadd_bound(double %initial, double %zero, i64 %remaining)
  %safe_one = call i1 @imre_exact_fadd_bound(double %initial, double %one, i64 %remaining)
  %safe_terms = and i1 %safe_zero, %safe_one
  %tripped = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %sparse_clear = icmp eq i8 %tripped, 0
  %safe = and i1 %safe_terms, %sparse_clear
  br i1 %safe, label %count_source, label %miss
count_source:
  br i1 %precounted, label %counts, label %orbit_count
orbit_count:
  %cycle_words_ptr = getelementptr i64, ptr %view, i64 3
  %cycle_words = load i64, ptr %cycle_words_ptr
  %repetitions = udiv i64 %remaining, %period
  %tail = urem i64 %remaining, %period
  %cycle_ones = call i64 @imre_bitmap_reverse_ones(ptr %bits, i64 %cycle_words, i64 %n, i64 %first, i64 %step, i64 %period, i64 %repetitions, i64 %tail)
  %abandoned = icmp slt i64 %cycle_ones, 0
  %after_count = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %pending_after = icmp ne i8 %after_count, 0
  %abort = or i1 %abandoned, %pending_after
  br i1 %abort, label %miss, label %counts
plan:
  ; Counting a tag also checks for interrupts. Require at most one omitted
  ; row per two visited rows before choosing the complement.
  %third = udiv i64 %n, 3
  %threshold = sub i64 %n, %third
  %enough = icmp uge i64 %remaining, %threshold
  %full_view = icmp sge i64 %first, 0
  ; A set bit costs one inverse modular lookup. Compare the scan and lookup
  ; costs to the number of probes on either side of the partial period.
  %rounded = add i64 %n, 63
  %word_count = lshr i64 %rounded, 6
  %max_ones = add i64 %prefix_ones, 1
  %lookups = shl i64 %max_ones, 4
  %budget = add i64 %word_count, %lookups
  %narrow = icmp ule i64 %n, 4294967296
  %room = icmp uge i64 %remaining, %budget
  %sparse = and i1 %narrow, %room
  %worthwhile = or i1 %enough, %sparse
  %long_scan = icmp uge i64 %remaining, 256
  %try_orbit = or i1 %worthwhile, %long_scan
  %candidate = and i1 %full_view, %try_orbit
  br i1 %candidate, label %orbit, label %miss
orbit:
  %proof = call { i64, i64 } @imre_modular_bezout(i64 %step, i64 %n)
  %gcd = extractvalue { i64, i64 } %proof, 0
  %permutation = icmp eq i64 %gcd, 1
  br i1 %permutation, label %whole_list, label %repeated
repeated:
  ; The modular index returns to its first row after n/gcd visits.
  %orbit_length = udiv exact i64 %n, %gcd
  %quarter = lshr i64 %remaining, 2
  %cheap = icmp ule i64 %orbit_length, %quarter
  br i1 %cheap, label %known_counts, label %miss
whole_list:
  %periods = udiv i64 %remaining, %n
  %whole = mul i64 %periods, %n
  %partial = sub i64 %remaining, %whole
  %omitted = sub i64 %n, %partial
  %shorter = icmp uge i64 %partial, %threshold
  %long_prefix = icmp uge i64 %partial, %budget
  %long_suffix = icmp uge i64 %omitted, %budget
  %both = and i1 %long_prefix, %long_suffix
  %invert = and i1 %narrow, %both
  %fold_partial = or i1 %shorter, %invert
  %bitmap_visits = select i1 %fold_partial, i64 %remaining, i64 %whole
  %exact_zero = call i1 @imre_exact_fadd_bound(double %initial, double %zero, i64 %bitmap_visits)
  %exact_one = call i1 @imre_exact_fadd_bound(double %initial, double %one, i64 %bitmap_visits)
  %exact = and i1 %exact_zero, %exact_one
  %eligible = and i1 %exact, %worthwhile
  br i1 %eligible, label %interrupt, label %miss
interrupt:
  %signal = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %clear = icmp eq i8 %signal, 0
  br i1 %clear, label %reduce, label %miss
reduce:
  %words_ptr = getelementptr i64, ptr %view, i64 3
  %words = load i64, ptr %words_ptr
  ; The existing prefix count excludes the physical last row.
  %last = sub i64 %n, 1
  %last_tag = call i1 @imre_bitmap_tag(ptr %bits, i64 %words, i64 %last)
  %extra = zext i1 %last_tag to i64
  %per_period = add i64 %prefix_ones, %extra
  %full_ones = mul i64 %periods, %per_period
  br i1 %invert, label %selected, label %direct
selected:
  %inverse = extractvalue { i64, i64 } %proof, 1
  %fits = icmp ult i64 %words, %word_count
  %stored = select i1 %fits, i64 %words, i64 %word_count
  %hits = call i64 @imre_bitmap_selected_ones(ptr %bits, i64 %stored, i64 %n, i64 %first, i64 %inverse, i64 %partial)
  br label %partial_count
direct:
  br i1 %shorter, label %complement, label %counts
complement:
  ; Walk backwards from the start of the next period to count the omitted rows.
  %absent = call i64 @imre_bitmap_reverse_ones(ptr %bits, i64 %words, i64 %n, i64 %first, i64 %step, i64 %omitted, i64 1, i64 0)
  br label %partial_count
partial_count:
  %speculative = phi i64 [ %absent, %complement ], [ %hits, %selected ]
  %complement_ones = sub i64 %per_period, %speculative
  %partial_ones = select i1 %invert, i64 %speculative, i64 %complement_ones
  %interrupted = icmp slt i64 %speculative, 0
  %included = add i64 %full_ones, %partial_ones
  br i1 %interrupted, label %miss, label %counts
counts:
  %visits = phi i64 [ %bitmap_visits, %direct ], [ %bitmap_visits, %partial_count ], [ %remaining, %count_source ], [ %remaining, %orbit_count ]
  %ones = phi i64 [ %full_ones, %direct ], [ %included, %partial_count ], [ %prefix_ones, %count_source ], [ %cycle_ones, %orbit_count ]
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

; Walk backwards from the first row. Over one orbit, left-1 is the forward
; visit number, so the first tail rows contribute one extra visit. An omitted
; suffix uses unit weights and no tail.
define internal i64 @imre_bitmap_reverse_ones(ptr %bits, i64 %words, i64 %n, i64 %first, i64 %step, i64 %length, i64 %periods, i64 %tail) nounwind {
entry:
  br label %loop
loop:
  %at = phi i64 [ %first, %entry ], [ %prev, %visit ]
  %left = phi i64 [ %length, %entry ], [ %rest, %visit ]
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
  %in_tail = icmp ule i64 %left, %tail
  %extra = zext i1 %in_tail to i64
  %weight = add i64 %periods, %extra
  %hits = select i1 %bit, i64 %weight, i64 0
  %added = add i64 %count, %hits
  %rest = sub i64 %left, 1
  %done = icmp eq i64 %rest, 0
  br i1 %done, label %exit, label %loop
interrupt:
  ret i64 -1
exit:
  ret i64 %added
}

; Multiplying a set bit's offset by the inverse stride gives its unique ordinal
; in the permutation. Both factors fit in 32 bits on this path.
define internal i64 @imre_bitmap_selected_ones(ptr %bits, i64 %words, i64 %n, i64 %first, i64 %inverse, i64 %partial) nounwind {
entry:
  %negative = icmp slt i64 %inverse, 0
  %positive = add i64 %inverse, %n
  %inv = select i1 %negative, i64 %positive, i64 %inverse
  br label %word
word:
  %index = phi i64 [ 0, %entry ], [ %next, %advance ]
  %count = phi i64 [ 0, %entry ], [ %subtotal, %advance ]
  %signal = load atomic volatile i8, ptr @imre_signal_tripped monotonic, align 1
  %pending = icmp ne i8 %signal, 0
  br i1 %pending, label %interrupt, label %read
read:
  %ptr = getelementptr inbounds i64, ptr %bits, i64 %index
  %tags = load i64, ptr %ptr
  %base = shl i64 %index, 6
  br label %test
test:
  %mask = phi i64 [ %tags, %read ], [ %rest, %visit ]
  %subtotal = phi i64 [ %count, %read ], [ %added, %visit ]
  %empty = icmp eq i64 %mask, 0
  br i1 %empty, label %advance, label %visit
visit:
  %bit = call i64 @llvm.cttz.i64(i64 %mask, i1 true)
  %physical = or i64 %base, %bit
  %delta = sub i64 %physical, %first
  %wrap = icmp ult i64 %physical, %first
  %wrapped = add i64 %delta, %n
  %offset = select i1 %wrap, i64 %wrapped, i64 %delta
  %product = mul i64 %offset, %inv
  %ordinal = urem i64 %product, %n
  %hit = icmp ult i64 %ordinal, %partial
  %one = zext i1 %hit to i64
  %added = add i64 %subtotal, %one
  %low = sub i64 %mask, 1
  %rest = and i64 %mask, %low
  br label %test
advance:
  %next = add i64 %index, 1
  %done = icmp eq i64 %next, %words
  br i1 %done, label %exit, label %word
interrupt:
  ret i64 -1
exit:
  ret i64 %subtotal
}

declare i64 @llvm.cttz.i64(i64, i1 immarg)

; A flat tag view supplies a physical prefix count. Sparse rows instead supply
; the exact visited count and two payloads, with a null tag pointer.
define internal i64 @imre_indexed_dictionary(ptr %list, ptr %view, i64 %start, i64 %stop, i64 %modulus, i64 %scale, i64 %offset) nounwind {
entry:
  %bitmap = call i64 @imre_struct_binary_prefix(ptr %list, ptr %view)
  %present = icmp sge i64 %bitmap, 0
  br i1 %present, label %done, label %bounds
bounds:
  %n = lshr i64 %modulus, 1
  %narrow = icmp ule i64 %n, 4294967296
  %distance = sub i64 %stop, %start
  %visits = lshr i64 %distance, 1
  %large = icmp uge i64 %visits, 256
  %eligible = and i1 %narrow, %large
  br i1 %eligible, label %orbit, label %done
orbit:
  %step = urem i64 %scale, %n
  %proof = call { i64, i64 } @imre_modular_bezout(i64 %step, i64 %n)
  %gcd = extractvalue { i64, i64 } %proof, 0
  %permutation = icmp eq i64 %gcd, 1
  br i1 %permutation, label %sparse, label %done
sparse:
  %coefficient = extractvalue { i64, i64 } %proof, 1
  %negative = icmp slt i64 %coefficient, 0
  %positive = add i64 %coefficient, %n
  %inverse = select i1 %negative, i64 %positive, i64 %coefficient
  %tag = call i64 @imre_bounded_modular_index(i64 %start, i64 %scale, i64 %offset, i64 %modulus)
  %first = lshr i64 %tag, 1
  %count = call i64 @imre_struct_sparse_indexed_count(ptr %list, ptr %view, i64 %n, i64 %first, i64 %inverse, i64 %visits)
  br label %done
done:
  %result = phi i64 [ %bitmap, %entry ], [ -1, %bounds ], [ -1, %orbit ], [ %count, %sparse ]
  ret i64 %result
}
