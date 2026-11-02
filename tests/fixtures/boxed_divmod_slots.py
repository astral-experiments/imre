def current():
    class Pair:
        def __divmod__(self, other):
            print("forward")
            return "pair"

        def __rdivmod__(self, other):
            print("reverse")
            return "rpair"

    class Child(Pair):
        def __rdivmod__(self, other):
            print("child reverse")
            return NotImplemented

    class Unchanged(Pair):
        pass

    class Declines:
        def __divmod__(self, other):
            return NotImplemented

        def __rdivmod__(self, other):
            raise AssertionError("same-type reflected operation ran")

    try:
        divmod(Declines(), Declines())
    except TypeError:
        pass
    else:
        raise AssertionError("same-type arithmetic must fail after a declined forward slot")

    assert divmod(Pair(), Pair()) == "pair"
    assert divmod(Pair(), Unchanged()) == "pair"
    assert divmod(Pair(), Child()) == "pair"
    assert divmod(object(), Pair()) == "rpair"

    class Forward(int):
        def __divmod__(self, other):
            print("declined")
            return NotImplemented

    class Reflected(int):
        def __rdivmod__(self, other):
            return "reflected"

    assert divmod(Forward(7), 3) == (2, 1)
    assert divmod(7, Reflected(3)) == "reflected"
    assert divmod(Forward(7), Reflected(3)) == "reflected"
    assert divmod(Reflected(7), Forward(3)) == (2, 1)
    try:
        divmod(Forward(7), Forward(3))
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("divmod reused an overridden integer slot")

    class Failing:
        def __divmod__(self, other):
            raise ValueError("failed inside __divmod__")

    try:
        divmod(Failing(), Pair())
    except ValueError as error:
        print(str(error))
    else:
        raise AssertionError("slot exception was lost")

    def reflect(other):
        return "reflected"

    class StaticParent:
        def __divmod__(self, other):
            return "forward"

        __rdivmod__ = staticmethod(reflect)

    class StaticChild(StaticParent):
        __rdivmod__ = staticmethod(reflect)

    assert divmod(StaticParent(), StaticChild()) == "forward"

    class ClassParent:
        def __divmod__(self, other):
            return "forward"

        @classmethod
        def __rdivmod__(cls, other):
            return cls

    class ClassChild(ClassParent):
        pass

    assert divmod(ClassParent(), ClassChild()) is ClassChild

    class Hook:
        def __call__(self, other):
            return "called"

        def __ne__(self, other):
            raise ValueError("slot comparison")

    class Custom(StaticParent):
        __rdivmod__ = Hook()

    class CustomChild(Custom):
        __rdivmod__ = Hook()

    try:
        divmod(Custom(), CustomChild())
    except ValueError as error:
        assert str(error) == "slot comparison"
    else:
        raise AssertionError("slot comparison error was lost")

    from operator import attrgetter
    getter = attrgetter("x")
    assert divmod(getter, Pair()) == "rpair"
    try:
        divmod(getter, 3)
    except TypeError:
        pass
    else:
        raise AssertionError("unsupported operand was accepted")

    class InstanceOnly:
        def __get__(self, instance, owner):
            if instance is None:
                raise ValueError("class lookup for a distinct native slot")
            return reflect

    class Base:
        pass

    class ChildOnly(Base):
        __rdivmod__ = InstanceOnly()

    class IntChild(int):
        __rdivmod__ = InstanceOnly()

    assert divmod(Base(), ChildOnly()) == "reflected"
    assert divmod(7, IntChild(3)) == "reflected"
