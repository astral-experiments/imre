from unittest import TestCase


case = TestCase()
compare = case.assertAlmostEqual
compare(float("inf"), float("inf"), places=3, delta=1)
compare(1.003, 1, places=2)
compare(100000000000000001, 100000000000000000, delta=1)

with case.assertRaisesRegex(TypeError, "specify delta or places not both"):
    compare(3, 4, places=2, delta=1)

for kwargs, expected in [
    ({}, "3 != 5 within 7 places (2 difference)"),
    ({"delta": 1}, "3 != 5 within 1 delta (2 difference)"),
]:
    with case.assertRaises(AssertionError) as caught:
        compare(3, 5, msg="detail", **kwargs)
    assert str(caught.exception) == expected + " : detail"


def current():
    compare = case.assertNotAlmostEqual
    compare(1.001, 1, places=3)
    compare(3, 5, delta=1)
    with case.assertRaisesRegex(TypeError, "specify delta or places not both"):
        compare(1, 1, places=2, delta=2)

    for kwargs, expected in [
        ({}, "3 == 3 within 7 places"),
        ({"delta": 0}, "3 == 3 within 0 delta (0 difference)"),
    ]:
        with case.assertRaises(AssertionError) as caught:
            compare(3, 3, msg="detail", **kwargs)
        assert str(caught.exception) == expected + " : detail"
