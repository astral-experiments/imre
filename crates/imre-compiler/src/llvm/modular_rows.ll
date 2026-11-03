; Solve scale*i + offset = index modulo n once for each exceptional row.
; Euclid's coefficients fit in i64 because the positive tagged modulus fits in 62 bits.
define internal { i64, i64 } @imre_modular_bezout(i64 %scale, i64 %n) nounwind {
entry:
  br label %loop
loop:
  %a = phi i64 [ %n, %entry ], [ %b, %step ]
  %b = phi i64 [ %scale, %entry ], [ %remainder, %step ]
  %x = phi i64 [ 0, %entry ], [ %y, %step ]
  %y = phi i64 [ 1, %entry ], [ %next, %step ]
  %done = icmp eq i64 %b, 0
  br i1 %done, label %result, label %step
step:
  %quotient = udiv i64 %a, %b
  %remainder = urem i64 %a, %b
  %product = mul i64 %quotient, %y
  %next = sub i64 %x, %product
  br label %loop
result:
  %gcd = insertvalue { i64, i64 } poison, i64 %a, 0
  %proof = insertvalue { i64, i64 } %gcd, i64 %x, 1
  ret { i64, i64 } %proof
}

; Describe the physical prefix [0, n) using the existing read-only span API.
; Uniform spans may share one payload. Record at most eight other physical indices.
; Inspect at most 17 spans (eight exceptions with a default gap on either side).
define internal i64 @imre_struct_modular_runs(ptr %list, i64 %tagged_n, i64 %scale, i64 %offset, ptr %plan) nounwind {
entry:
  %span = alloca { i64, i64, i64, ptr }, align 8
  %length_slot = getelementptr i64, ptr %span, i64 1
  %mask_slot = getelementptr i64, ptr %span, i64 2
  %data_slot = getelementptr i64, ptr %span, i64 3
  %indices = getelementptr i64, ptr %plan, i64 1
  %n = lshr i64 %tagged_n, 1
  %stride = urem i64 %scale, %n
  %constant = icmp eq i64 %stride, 0
  br i1 %constant, label %uniform, label %scan
scan:
  %at = phi i64 [ 0, %entry ], [ %end, %more ]
  %count = phi i64 [ 0, %entry ], [ %used, %more ]
  %remaining = phi i64 [ 17, %entry ], [ %left, %more ]
  %default = phi ptr [ null, %entry ], [ %anchor, %more ]
  call void @imre_struct_seek_span(ptr %list, i64 %at, ptr %span)
  %start = load i64, ptr %span
  %length = load i64, ptr %length_slot
  %mask = load i64, ptr %mask_slot
  %data = load ptr, ptr %data_slot
  %span_end = add i64 %start, %length
  %within = icmp ult i64 %span_end, %n
  %end = select i1 %within, i64 %span_end, i64 %n
  %broadcast = icmp eq i64 %mask, 0
  %first = icmp eq ptr %default, null
  %establish = and i1 %broadcast, %first
  %anchor = select i1 %establish, ptr %data, ptr %default
  %same = icmp eq ptr %data, %anchor
  %common = and i1 %broadcast, %same
  br i1 %common, label %advance, label %exception
exception:
  %size = sub i64 %end, %at
  %total = add i64 %count, %size
  %fits = icmp ule i64 %total, 8
  br i1 %fits, label %record, label %ordinary
record:
  %index = phi i64 [ %at, %exception ], [ %following, %record ]
  %slot = phi i64 [ %count, %exception ], [ %next_slot, %record ]
  %dest = getelementptr i64, ptr %indices, i64 %slot
  store i64 %index, ptr %dest
  %following = add i64 %index, 1
  %next_slot = add i64 %slot, 1
  %last = icmp eq i64 %following, %end
  br i1 %last, label %advance, label %record
advance:
  %used = phi i64 [ %count, %scan ], [ %total, %record ]
  %finished = icmp eq i64 %end, %n
  br i1 %finished, label %collected, label %more
more:
  %left = sub i64 %remaining, 1
  %budget = icmp ne i64 %left, 0
  br i1 %budget, label %scan, label %ordinary
collected:
  %empty = icmp eq i64 %used, 0
  br i1 %empty, label %uniform, label %solve
solve:
  %proof = call { i64, i64 } @imre_modular_bezout(i64 %stride, i64 %n)
  %gcd = extractvalue { i64, i64 } %proof, 0
  %inverse = extractvalue { i64, i64 } %proof, 1
  %period = udiv i64 %n, %gcd
  store i64 %period, ptr %plan
  %wide_inverse = sext i64 %inverse to i128
  %wide_period = zext i64 %period to i128
  br label %map
map:
  %read = phi i64 [ 0, %solve ], [ %next_read, %mapped ]
  %write = phi i64 [ 0, %solve ], [ %written, %mapped ]
  %src = getelementptr i64, ptr %indices, i64 %read
  %physical = load i64, ptr %src
  %delta = sub i64 %physical, %offset
  %residue = srem i64 %delta, %gcd
  %reachable = icmp eq i64 %residue, 0
  br i1 %reachable, label %keep, label %mapped
keep:
  ; Compact reachable residues in place. The product of two tagged integers fits i128.
  %q = sdiv exact i64 %delta, %gcd
  %wide_q = sext i64 %q to i128
  %wide_product = mul i128 %wide_q, %wide_inverse
  %wide_rem = srem i128 %wide_product, %wide_period
  %rem = trunc i128 %wide_rem to i64
  %negative = icmp slt i64 %rem, 0
  %positive = add i64 %rem, %period
  %phase = select i1 %negative, i64 %positive, i64 %rem
  %dst = getelementptr i64, ptr %indices, i64 %write
  store i64 %phase, ptr %dst
  %next_write = add i64 %write, 1
  br label %mapped
mapped:
  %written = phi i64 [ %write, %map ], [ %next_write, %keep ]
  %next_read = add i64 %read, 1
  %complete = icmp eq i64 %next_read, %used
  br i1 %complete, label %result, label %map
result:
  ret i64 %written
uniform:
  ret i64 0
ordinary:
  ret i64 -1
}

; An exceptional visit executes alone. Every preceding visit reads the default.
define internal i64 @imre_struct_modular_span(ptr %plan, i64 %count, i64 %ordinal, i64 %limit) alwaysinline nounwind {
entry:
  %period = load i64, ptr %plan
  %current = urem i64 %ordinal, %period
  %indices = getelementptr i64, ptr %plan, i64 1
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %loop ]
  %stop = phi i64 [ %limit, %entry ], [ %shorter, %loop ]
  %slot = getelementptr i64, ptr %indices, i64 %i
  %phase = load i64, ptr %slot
  %distance = sub i64 %phase, %current
  %wraps = icmp ult i64 %phase, %current
  %wrapped = add i64 %distance, %period
  %length = select i1 %wraps, i64 %wrapped, i64 %distance
  %nearer = icmp ult i64 %length, %stop
  %shorter = select i1 %nearer, i64 %length, i64 %stop
  %next = add i64 %i, 1
  %last = icmp eq i64 %next, %count
  br i1 %last, label %done, label %loop
done:
  %hit = icmp eq i64 %shorter, 0
  %span = select i1 %hit, i64 1, i64 %shorter
  ret i64 %span
}
