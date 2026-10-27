from _weakref import getweakrefs, ref


class Referent:
    pass


class Tagged(ref):
    __slots__ = "tag", "__weakref__"

    def __new__(cls, target, callback, tag):
        result = ref.__new__(cls, target, callback)
        result.tag = tag
        return result

    def __init__(self, target, callback, tag):
        super().__init__(target, callback)

    def __call__(self):
        return super().__call__()


class Plain(ref):
    pass


def current():
    target = Referent()
    calls = []
    callback = calls.append
    tagged = Tagged(target, callback, "owner")
    print(target.__weakref__ is tagged)
    plain = Plain(target)
    plain2 = Plain(target)
    canonical = ref(target)
    print(type(tagged) is Tagged, isinstance(tagged, ref), tagged.tag, tagged() is target)
    print(plain is not plain2, ref(target) is canonical, ref.__eq__(plain, canonical))
    print(tagged.__callback__ is callback, plain.__callback__ is None)
    try:
        tagged.__callback__ = None
    except AttributeError as exc:
        print(str(exc))
    refs = getweakrefs(target)
    print(len(refs), refs[0] is canonical, refs[1] is plain2, refs[2] is plain, refs[3] is tagged)
    observed = ref(tagged)
    print(target.__weakref__ is canonical, tagged.__weakref__ is observed)
    del target
    print(tagged() is None, plain() is None, calls == [tagged], observed() is tagged)
    print(tagged.__callback__ is None)
    try:
        ref.__new__(object, tagged)
    except TypeError as exc:
        print(str(exc))
    try:
        ref.__new__(1, tagged)
    except TypeError as exc:
        print(str(exc))
