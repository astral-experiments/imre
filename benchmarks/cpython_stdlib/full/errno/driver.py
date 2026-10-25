"""Schedule original unittest method bodies; leave the complete test module unchanged."""
from errno import EDOM, errorcode
from suite import ErrnoAttributeTests, ErrorcodeTests
import sys


def fail_import(name: str | None, spelling: str | None) -> None:
    raise ImportError("cannot import", name=name, name_from=spelling)


def run(mode: str) -> None:
    if mode == "test_for_improper_attributes":
        ErrnoAttributeTests().test_for_improper_attributes()
    elif mode == "test_using_errorcode":
        ErrnoAttributeTests().test_using_errorcode()
    elif mode == "test_attributes_in_errorcode":
        ErrorcodeTests().test_attributes_in_errorcode()
    elif mode == "optional-hint":
        fail_import("errno", "EDAOM")
    elif mode == "optional-module":
        fail_import(None, "EDAOM")
    elif mode == "optional-spelling":
        fail_import("errno", None)
    elif mode == "caught-import":
        try:
            raise ImportError("cannot import", name="errno", name_from="EDAOM")
        except ImportError as exc:
            print(str(exc))
            print(repr(exc))
            print(repr(exc.args))
            print(str(exc.name))
            print(str(exc.name_from))
    elif mode == "missing-attribute":
        errorcode[-1234] = "MISSING_ERRNO_PROBE"
        ErrnoAttributeTests().test_using_errorcode()
    elif mode == "missing-code":
        del errorcode[EDOM]
        ErrorcodeTests().test_attributes_in_errorcode()
    else:
        raise ValueError(mode)
    print("pass")


try:
    run(sys.argv[1])
except AssertionError as exc:
    print("assertion: " + str(exc))
