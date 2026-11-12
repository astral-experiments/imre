import copy
class Base:
    def __reduce_ex__(self, protocol: int) -> int: return 0
class Child(Base): pass
copy.copy(Child())
