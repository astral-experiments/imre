class Base:
    @classmethod
    def pick(cls, n: int = 21) -> int:
        return n + 1


class Child(Base):
    pass


saved = Child.pick
raw = saved.__func__
print(saved(), raw(Child))
raw.__defaults__ = (41,)
print(saved(), raw(Base), Child.pick(), saved(8))
