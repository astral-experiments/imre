; Tagged residues multiply exactly in i128. Other operands use Python's
; arbitrary-precision power, including modular inversion and zero-modulus errors.
define internal i64 @imre_tagged_modpow(i64 %base, i64 %exponent, i64 %modulus) alwaysinline {
entry:
  %tags.be = and i64 %base, %exponent
  %tags.all = and i64 %tags.be, %modulus
  %tagged = trunc i64 %tags.all to i1
  %nonnegative = icmp sgt i64 %exponent, 0
  %nonzero = icmp ne i64 %modulus, 1
  %valid = and i1 %nonnegative, %nonzero
  %eligible = and i1 %tagged, %valid
  br i1 %eligible, label %words, label %fallback

fallback:
  %big = call i64 @imre_int_modpow(i64 %base, i64 %exponent, i64 %modulus)
  ret i64 %big

words:
  %m = ashr i64 %modulus, 1
  %negative = icmp slt i64 %m, 0
  %minus.m = sub nsw i64 0, %m
  %magnitude = select i1 %negative, i64 %minus.m, i64 %m
  %unit = icmp eq i64 %magnitude, 1
  br i1 %unit, label %zero, label %start

zero:
  ret i64 1

start:
  %b = ashr i64 %base, 1
  %rem = srem i64 %b, %magnitude
  %rem.negative = icmp slt i64 %rem, 0
  %adjusted = add nsw i64 %rem, %magnitude
  %initial = select i1 %rem.negative, i64 %adjusted, i64 %rem
  %initial.factor = zext i64 %initial to i128
  %wide.magnitude = zext i64 %magnitude to i128
  %e = lshr i64 %exponent, 1
  br label %bits

bits:
  %remaining = phi i64 [ %e, %start ], [ %next, %square ]
  %factor = phi i128 [ %initial.factor, %start ], [ %squared, %square ]
  %acc = phi i128 [ 1, %start ], [ %result, %square ]
  %odd = trunc i64 %remaining to i1
  br i1 %odd, label %multiply, label %shift

multiply:
  %product = mul nuw i128 %acc, %factor
  %reduced = urem i128 %product, %wide.magnitude
  br label %shift

shift:
  %result = phi i128 [ %reduced, %multiply ], [ %acc, %bits ]
  %next = lshr i64 %remaining, 1
  %done = icmp eq i64 %next, 0
  br i1 %done, label %finish, label %square

square:
  %factor.product = mul nuw i128 %factor, %factor
  %squared = urem i128 %factor.product, %wide.magnitude
  br label %bits

finish:
  %word = trunc i128 %result to i64
  %has.remainder = icmp ne i64 %word, 0
  %adjust = and i1 %negative, %has.remainder
  %signed.rem = add nsw i64 %word, %m
  %value = select i1 %adjust, i64 %signed.rem, i64 %word
  %shifted = shl i64 %value, 1
  %tag = or i64 %shifted, 1
  ret i64 %tag
}
