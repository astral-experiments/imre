import functools
from annotationlib import get_annotations
from functools import lru_cache

def example(first: int) -> None:
    @lru_cache(maxsize=2)
    def value(arg: int):
        print("miss")
        print(arg)
        return arg + 1

    old = value
    print(value(first))
    print(old(first))
    print(repr(value.cache_parameters()))

    @lru_cache(maxsize=1000, typed=True)
    def value(arg: int):
        print("new")
        print(arg)
        return arg + 10

    print(value(first))
    print(value(first))
    print(old(first))
    print(repr(old.cache_parameters()))
    print(repr(value.cache_parameters()))
    print(old.cache_info()[0])
    print(value.cache_info()[0])
    value.cache_clear()
    print(old.cache_info()[3])
    print(value.cache_info()[3])
    print(sum(map(value, [first, first])))
    print(value.cache_info()[0])
    print(repr(get_annotations(value)))

class Library:
    module = functools

    def sample(self) -> None:
        @self.module.cache
        def constant():
            return 7

        print(constant())
        print(constant())
        print(repr(constant.cache_parameters()))

example(4)
example(9)
Library().sample()
