import builtins
from functools import partial, singledispatch
def exercise() -> None:
    @singledispatch
    def ignored(unread):
        return "base"
    words = ("étoile",)
    live = builtins.len
    print(partial(live, *words)())
    print(partial(*(live, "ab"))())
    print(partial(ignored, *words)())
    print(partial(*(live, [0, 1]), *())())
exercise()
