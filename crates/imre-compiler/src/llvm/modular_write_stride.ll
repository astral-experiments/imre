; A backward progression has the same row indices as its nonnegative stride
; modulo n. Keep the original stride when the scalar multiply could box.
define internal i64 @imre_modular_write_stride(i64 %stride, i64 %stop, i64 %modulus) alwaysinline nounwind {
entry:
  %negative = icmp slt i64 %stride, 0
  br i1 %negative, label %check, label %original

check:
  %tags = and i64 %stride, %modulus
  %integers = trunc i64 %tags to i1
  %positive = icmp sgt i64 %modulus, 1
  %valid = and i1 %integers, %positive
  ; The stride minus its tag is twice its payload. This product fits i64
  ; exactly when the source product fits a tagged integer. The store kernel
  ; checks the ordinal bounds and the nonnegative offset before any writes.
  %twice_stride = sub i64 %stride, 1
  %end = ashr i64 %stop, 1
  %last = sub i64 %end, 1
  %product = call { i64, i1 } @llvm.smul.with.overflow.i64(i64 %last, i64 %twice_stride)
  %overflow = extractvalue { i64, i1 } %product, 1
  %fits = xor i1 %overflow, true
  %can_reduce = and i1 %valid, %fits
  br i1 %can_reduce, label %reduce, label %original

reduce:
  %n = lshr i64 %modulus, 1
  %step = ashr i64 %stride, 1
  %remainder = srem i64 %step, %n
  %zero = icmp eq i64 %remainder, 0
  %wrapped = add nsw i64 %remainder, %n
  %forward = select i1 %zero, i64 0, i64 %wrapped
  %shifted = shl nuw nsw i64 %forward, 1
  %tagged = or i64 %shifted, 1
  ret i64 %tagged

original:
  ret i64 %stride
}
