def observe(n: int) -> int:
    print(n)
    return n


print(2**128)
print((-2)**129)
print(0**0)
print((-1)**340282366920938463463374607431768211457)
print(True**False)
print(observe(-2)**-3)
print(3**abs(observe(-4)))
print(pow(3, 72, None))
print(pow(base=2, exp=-4))
print(pow(mod=observe(-13), exp=observe(-3), base=observe(2)))
print(pow(0, -9, 1))
print(pow(0, -9, -1))
print(pow(4, 0, -9))

# The same integer operations used to check sys.hash_info.modulus in test_sys.
prime = 2305843009213693951
print(pow(2 - prime, 61, -prime))
print(pow(-(1 << 62), 2, -(1 << 62)))
for x in range(1, 100):
    if pow(x, prime - 1, prime) != 1:
        raise AssertionError(x)

# Squaring a 32-bit residue can use all 64 bits of the product.
assert pow(4294967293, (1 << 75) + 3, 4294967295) == 4294967287
assert pow(-17, (1 << 75) + 3, -4294967294) == -963318497

for modulus in [0, -4, 18]:
    try:
        print(pow(6, -1, modulus))
    except ValueError as error:
        print(str(error))

try:
    print(0**-1)
except ZeroDivisionError as error:
    print(str(error))
