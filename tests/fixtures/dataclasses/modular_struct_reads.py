from imre import struct


@struct
class Reading:
    value: float


def total(rows: list[Reading], stop: int, modulus: int) -> float:
    result = 0.0
    for i in range(3, stop):
        at = (6 * i + 5) % modulus
        result += rows[at].value
    return result


def main() -> None:
    base = Reading(0.25)
    rows = [base] * 512
    for i in range(10):
        # With modulus 504 only one in six physical indices is reachable.
        print(total(rows, 2048, 504))
        print(total(rows, 2048, 511))
        rows[(5 + 67 * i) % 512] = Reading(float(i + 2))
    rows[5] = base
    print(total(rows, 4096, 504))
    print(total(rows, 0, 511))


main()
