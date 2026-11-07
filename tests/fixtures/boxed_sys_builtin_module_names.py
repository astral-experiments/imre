import sys
import unittest


def current():
    names = vars(sys)["builtin_module_names"]
    from sys import builtin_module_names

    assert type(names) is tuple and names == tuple(sorted(set(names)))
    assert names is builtin_module_names is sys.builtin_module_names
    assert {"builtins", "errno", "sys"}.issubset(names)
    assert "_frozen_importlib" not in names and "json" not in names

    with unittest.TestCase().assertRaisesRegex(
        TypeError, "Can not use builtin modules as dotted module names"
    ):
        unittest.TestLoader().discover("sys")


if __name__ == "__main__":
    current()
