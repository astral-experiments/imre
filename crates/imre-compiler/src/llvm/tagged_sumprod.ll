; A checked prefix accumulates exact tagged products before the first wide operand or sum.
; The lists are rooted throughout. Exact integer arithmetic cannot call Python or resize them.
declare { i128, i1 } @llvm.sadd.with.overflow.i128(i128, i128)

define internal i64 @imre_sumprod_pack_total(i128 %value) {
entry:
  %word = trunc i128 %value to i64
  %tag = shl i64 %word, 1
  %small = or i64 %tag, 1
  %decoded = ashr i64 %small, 1
  %check = sext i64 %decoded to i128
  %fits = icmp eq i128 %check, %value
  br i1 %fits, label %done, label %wide
done:
  ret i64 %small
wide:
  ; Reconstruct a signed high digit and two unsigned 62-bit digits using the
  ; existing integer ABI. Each fixed shift and addition is exact and bounded.
  %top = ashr i128 %value, 124
  %top64 = trunc i128 %top to i64
  %top2 = shl i64 %top64, 1
  %top_word = or i64 %top2, 1
  br label %digit
digit:
  %shift = phi i128 [ 62, %wide ], [ 0, %digit ]
  %acc = phi i64 [ %top_word, %wide ], [ %sum, %digit ]
  %bits = lshr i128 %value, %shift
  %bits64 = trunc i128 %bits to i64
  %low = and i64 %bits64, 4611686018427387903
  %low2 = shl i64 %low, 1
  %low_word = or i64 %low2, 1
  %grown = call i64 @imre_int_binary(i64 %acc, i64 125, i32 8)
  call void @imre_int_release(i64 %acc)
  %sum = call i64 @imre_int_binary(i64 %grown, i64 %low_word, i32 0)
  call void @imre_int_release(i64 %grown)
  %last = icmp eq i128 %shift, 0
  br i1 %last, label %packed, label %digit
packed:
  ret i64 %sum
}

define internal i64 @imre_tagged_sumprod(ptr %left, ptr %right) {
entry:
  %left_len = call i64 @imre_list_len(ptr %left)
  %right_len = call i64 @imre_list_len(ptr %right)
  %equal = icmp eq i64 %left_len, %right_len
  br i1 %equal, label %arrays, label %fallback
arrays:
  %len = lshr i64 %left_len, 1
  %a = call ptr @imre_list_data_readonly(ptr %left)
  %b = call ptr @imre_list_data_readonly(ptr %right)
  br label %prefix
prefix:
  %i = phi i64 [ 0, %arrays ], [ %next, %advance ]
  %total = phi i128 [ 0, %arrays ], [ %updated, %advance ]
  %finished = icmp eq i64 %i, %len
  br i1 %finished, label %result, label %pair
pair:
  %ap = getelementptr inbounds i64, ptr %a, i64 %i
  %bp = getelementptr inbounds i64, ptr %b, i64 %i
  %x = load i64, ptr %ap
  %y = load i64, ptr %bp
  %tags = and i64 %x, %y
  %small = trunc i64 %tags to i1
  br i1 %small, label %product, label %promote
product:
  %sx = ashr i64 %x, 1
  %sy = ashr i64 %y, 1
  %wx = sext i64 %sx to i128
  %wy = sext i64 %sy to i128
  %term = mul nsw i128 %wx, %wy
  %checked = call { i128, i1 } @llvm.sadd.with.overflow.i128(i128 %total, i128 %term)
  %updated = extractvalue { i128, i1 } %checked, 0
  %overflow = extractvalue { i128, i1 } %checked, 1
  br i1 %overflow, label %promote, label %advance
advance:
  %next = add nuw i64 %i, 1
  br label %prefix
result:
  %answer = call i64 @imre_sumprod_pack_total(i128 %total)
  ret i64 %answer
promote:
  %first = icmp eq i64 %i, 0
  br i1 %first, label %fallback, label %suffix_start
fallback:
  ; The shared reducer also preserves pair evaluation before a length error.
  %generic = call i64 @imre_sumprod_int_lists(ptr %left, ptr %right)
  ret i64 %generic
suffix_start:
  %initial = call i64 @imre_sumprod_pack_total(i128 %total)
  br label %suffix
suffix:
  %j = phi i64 [ %i, %suffix_start ], [ %after, %suffix ]
  %running = phi i64 [ %initial, %suffix_start ], [ %added, %suffix ]
  %xp = getelementptr inbounds i64, ptr %a, i64 %j
  %yp = getelementptr inbounds i64, ptr %b, i64 %j
  %lhs = load i64, ptr %xp
  %rhs = load i64, ptr %yp
  %term_word = call i64 @imre_int_binary(i64 %lhs, i64 %rhs, i32 2)
  %added = call i64 @imre_int_binary(i64 %running, i64 %term_word, i32 0)
  call void @imre_int_release(i64 %term_word)
  call void @imre_int_release(i64 %running)
  %after = add nuw i64 %j, 1
  %end = icmp eq i64 %after, %len
  br i1 %end, label %last, label %suffix
last:
  ret i64 %added
}
