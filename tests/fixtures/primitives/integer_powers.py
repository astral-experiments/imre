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
for x in range(1, 100):
    if pow(x, prime - 1, prime) != 1:
        raise AssertionError(x)

for modulus in [0, -4, 18]:
    try:
        print(pow(6, -1, modulus))
    except ValueError as error:
        print(str(error))

try:
    print(0**-1)
except ZeroDivisionError as error:
    print(str(error))
