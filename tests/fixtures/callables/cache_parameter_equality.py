from functools import lru_cache


def run() -> None:
    def f():
        return 1

    actual = lru_cache(2)(f).cache_parameters()
    reverse = {"typed": False, "maxsize": 2}
    print(actual == reverse)
    print(actual == {"maxsize": 3, "typed": False})
    print(actual == {"maxsize": 2, "other": False})
    actual["typed"] = True
    print(actual == reverse)
    actual["typed"] = 1.0
    print(actual == {"typed": True, "maxsize": 2})
    actual["typed"] = float("nan")
    print(actual == {"typed": True, "maxsize": 2})

    numeric = {"digit": True, "rest": "one"}
    print(numeric == {"rest": "one", "digit": 1})
    print(numeric == {"rest": "one", "digit": 2})
    print(repr(numeric))
    print(repr({"negative": -5, "void": None, "large": 922337203685477580812345}))


run()
