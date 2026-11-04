from fractions import Fraction
from numbers import Number, Rational

for text in (" -18 / 24 ", "١_٢.５e-1", "-0.00", "+2_100", ".025", "2.e5"):
    value = Fraction(text)
    print(repr(value))
    print(str(value))
    print(value.as_integer_ratio())
    print(value.is_integer())

for numerator, denominator in ((1, 4), (3, 4), (-3, 2), (-5, 2), (47, -19),
                               (2**201 + 33, 2**167 + 13)):
    value = Fraction(numerator, denominator)
    for bound in (1, 2, 3, 10, 1000, 2**180):
        print(value.limit_denominator(bound).as_integer_ratio())
    print(Fraction(value) is value)

for text in ("1.2_3/7", "1e+_5", "²/4", "1/__4"):
    try:
        Fraction(text)
    except ValueError as error:
        print(str(error))
try:
    Fraction(13, 0)
except ZeroDivisionError as error:
    print(str(error))

for value in (Fraction(1, 2), Fraction(0)):
    print(isinstance(value, Number))
    print(isinstance(value, Rational))
print(isinstance(0.5, Rational))
print(isinstance(False, Rational))

print(Fraction.__module__)
prime = 2**61 - 1
for numerator, denominator in ((0, 1), (-1, 1), (11, -7), (9, prime),
                               (-9, prime), (2**313 + 17, 2**201 - 7)):
    print(hash(Fraction(numerator, denominator)))
