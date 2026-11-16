import copy
class C:
    def __getstate__(self) -> int: return 0
copy.copy(C())
