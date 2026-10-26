"""Each returned attribute or tuple slice is evaluated in the native program."""
import sys
from importlib.util import cache_from_source

def main() -> None:
    impl = sys.implementation
    version = impl.version
    name = sys.argv[1]
    if name == "identity":
        print(impl is sys.implementation)
        print(version is sys.implementation.version)
        print(version is not sys.version_info)
        print(repr(impl))
    elif name == "has":
        print(repr(hasattr(impl, sys.argv[2])))
    elif name == "name":
        print(repr(impl.name))
    elif name == "cache_tag":
        print(repr(impl.cache_tag))
    elif name == "cache_alias":
        tag = sys.argv[2]
        sys.implementation.cache_tag = tag
        current = impl.cache_tag
        print(current is not None and current is tag)
        print("cache_tag='shared-cache-unit'" in repr(impl))
        print(cache_from_source("pkg/file.py"))
        impl.cache_tag = None
        print(sys.implementation.cache_tag is None)
        print("cache_tag=None" in repr(impl))
        try:
            cache_from_source("pkg/file.py")
        except NotImplementedError:
            print("no cache")
    elif name == "hexversion":
        print(repr(impl.hexversion))
    elif name == "supports_isolated_interpreters":
        print(repr(impl.supports_isolated_interpreters))
    elif name == "version":
        print(repr(version[:]))
    elif name == "prefix":
        print(repr(version[:2]))
    elif name == "major":
        print(repr(version.major))
    elif name == "minor":
        print(repr(version.minor))
    elif name == "micro":
        print(repr(version.micro))
    elif name == "releaselevel":
        print(repr(version.releaselevel))
    elif name == "serial":
        print(repr(version.serial))
    elif name == "language":
        print(repr(sys.version_info[:]))
        print(repr(sys.hexversion))
        print(repr(sys.version))
    else:
        raise ValueError("unknown metadata operation")

main()
