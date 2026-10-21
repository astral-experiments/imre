"""Exercise optimized constant divisors around tagged and arbitrary precision bounds."""


def main() -> None:
    values = [
        -170141183460469231731687303715884105728,
        -4611686018427387905,
        -4611686018427387904,
        -4611686018427387903,
        -4294967297,
        -9,
        -7,
        -2,
        -1,
        0,
        1,
        2,
        7,
        9,
        4294967297,
        4611686018427387903,
        4611686018427387904,
        170141183460469231731687303715884105728,
    ]
    for position in range(len(values)):
        value = values[position]
        print(value // 1)
        print(value % 1)
        print(value // 2)
        print(value % 2)
        print(value // -2)
        print(value % -2)
        print(value // -1)
        print(value % -1)
        print(value // 9)
        print(value % 9)
        print(value // -9)
        print(value % -9)
        print(value // 4294967296)
        print(value % 4294967296)
        print(value // -4294967296)
        print(value % -4294967296)
        print(value // -4611686018427387904)
        print(value % -4611686018427387904)
        print(value // 4611686018427387903)
        print(value % 4611686018427387903)


main()
