events = []


def observe(label):
    events.append(label)
    return label


class Outer:
    first = observe("outer before")

    class Inner:
        value = observe("inner body")

    last = observe("outer after")


assert events == ["outer before", "inner body", "outer after"]
assert Outer.Inner.__name__ == "Inner"
assert Outer.Inner.__qualname__ == "Outer.Inner"
assert Outer.Inner.value == "inner body"
print(Outer.Inner.__qualname__, events)
