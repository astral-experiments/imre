import sys
from importlib.util import cache_from_source


def accepted(value: str) -> bool:
    try:
        path = cache_from_source("x.py", optimization=value)
    except ValueError:
        return False
    if path != "__pycache__/x.probe.opt-" + value + ".pyc":
        raise AssertionError("accepted filename changed")
    return True


def ranges() -> None:
    sys.implementation.cache_tag = "probe"
    sys.pycache_prefix = None
    previous = False
    for cp in range(1114112):
        if 55296 <= cp and cp < 57344:
            continue
        ok = accepted(chr(cp))
        if ok != previous:
            print(cp)
            previous = ok


ranges()
