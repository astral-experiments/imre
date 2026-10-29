def current():
    suffix = ".x"
    for _ in range(15):
        suffix = suffix + suffix
    for root in ("missing_relar_probe", "sys"):
        try:
            __import__(root + suffix)
        except RecursionError:
            print("recursion")
    try:
        __import__("sys.missing")
    except ModuleNotFoundError:
        print("missing")
