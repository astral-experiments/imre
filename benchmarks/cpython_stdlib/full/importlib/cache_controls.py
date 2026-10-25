"""Observe native input evaluation, warning order, cache settings, and path spellings."""
import sys
import warnings
from importlib import util
from importlib.machinery import ModuleSpec
from pathlib import PurePath
from sys import implementation as impl


class ChangingPath:
    def __fspath__(self) -> str:
        print("fspath")
        impl.cache_tag = "from-path"
        return "/root/package.py"


class ChangingOptimization:
    def __str__(self) -> str:
        print("str")
        impl.cache_tag = "changed-too-late"
        sys.pycache_prefix = "/new-prefix"
        return "中文²"


class FailingPath:
    def __fspath__(self) -> str:
        print("fspath-error")
        raise ValueError("bad path")


class FailingTruth:
    def __bool__(self):
        print("bool-error")
        raise RuntimeError("bad truth")


def check(tag: str, prefix: str) -> None:
    impl.cache_tag = tag
    sys.pycache_prefix = prefix
    print(impl.cache_tag is tag)
    print(sys.pycache_prefix is prefix)
    old_tag = impl.cache_tag
    old_prefix = sys.pycache_prefix
    print(util.cache_from_source(ChangingPath(), optimization=ChangingOptimization()))
    print(old_tag is tag)
    print(old_prefix is prefix)
    print(impl.cache_tag == "changed-too-late")
    print(sys.pycache_prefix == "/new-prefix")
    impl.cache_tag = None
    sys.pycache_prefix = None
    try:
        print(util.cache_from_source(FailingPath()))
    except ValueError as exc:
        print(str(exc))
    try:
        print(util.source_from_cache(FailingPath()))
    except NotImplementedError as exc:
        print(str(exc))
    for reverse in [False, True]:
        try:
            if reverse:
                print(util.source_from_cache(b"__pycache__/x.tag.pyc"))
            else:
                print(util.cache_from_source(b"x.py"))
        except (TypeError, NotImplementedError) as exc:
            print(repr(exc))
    warnings.simplefilter("error", DeprecationWarning)
    try:
        print(util.cache_from_source(FailingPath(), FailingTruth(), optimization="x"))
    except Warning as exc:
        print(repr(exc))
    warnings.simplefilter("ignore", DeprecationWarning)
    try:
        print(util.cache_from_source(FailingPath(), FailingTruth(), optimization="x"))
    except TypeError as exc:
        print(repr(exc))
    try:
        print(util.cache_from_source(FailingPath(), FailingTruth()))
    except RuntimeError as exc:
        print(repr(exc))
    impl.cache_tag = tag
    for opt in ["田²", "¹", "𞅀", "ß", "é", "a-b", ".", "", "42", "🐍"]:
        try:
            print(util.cache_from_source(".//one/../two/.hidden", optimization=opt))
        except ValueError as exc:
            print(str(exc))
    spec = ModuleSpec("a", None, origin="/pkg/a.py")
    spec.has_location = True
    saved = spec.cached
    print(spec.cached is saved)
    sys.pycache_prefix = prefix
    print(spec.cached is saved)
    spec.cached = None
    now = spec.cached
    if now is not None:
        print(now)
    print(util.source_from_cache(PurePath(prefix, "pkg", "a.tag.opt-².pyc")))
    print(sys.flags.optimize)
    warnings.resetwarnings()
    warnings.simplefilter("always", DeprecationWarning)
    print(util.cache_from_source("end.py", True))


check(input(), input())
