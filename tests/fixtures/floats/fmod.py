from math import fmod


def divisor() -> float:
    print("evaluated divisor")
    return 3.0


print(fmod(-360.0, 360.0), fmod(7.0, -4.0), fmod(True, 2))
print(fmod(-1e-100, 1e100), fmod(1.7976931348623157e308, 3.0))
try:
    print(fmod(10**400, divisor()))
except OverflowError as exc:
    print(str(exc))
try:
    print(fmod(2, 0))
except ValueError as exc:
    print(str(exc))
