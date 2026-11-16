from inspect import Attribute, classify_class_attrs, getclasstree, getmembers, getmembers_static


class Members:
    def __init__(self):
        self.reads = 0

    def __dir__(self):
        return ["value", "missing", "method", "value"]

    def method(self):
        pass

    @property
    def value(self):
        self.reads += 1
        return 42


def reject(value):
    raise AttributeError("predicate failed")


def current():
    subject = Members()
    assert getmembers_static(subject) == [("method", Members.method), ("value", Members.value)]
    assert subject.reads == 0
    assert getmembers(subject) == [("method", subject.method), ("value", 42)]
    assert subject.reads == 2
    try:
        getmembers_static(subject, reject)
    except AttributeError as exc:
        assert exc.args == ("predicate failed",)
    else:
        raise AssertionError("predicate exception was swallowed")
    classify_inherited()
    class_tree()


def classify_inherited():
    reads = []

    class Parent(Members):
        @property
        def unavailable(self):
            reads.append(self)
            raise AttributeError("unavailable")

    class Child(Parent):
        pass

    attrs = {row.name: row for row in classify_class_attrs(Child)}
    for name, kind, owner in (("unavailable", "property", Parent), ("method", "method", Members)):
        row = attrs[name]
        assert type(row) is Attribute
        assert row.kind == kind and row.defining_class is owner
        assert row.object is owner.__dict__[name]
    assert not reads


def class_tree():
    class Left:
        pass

    class Right:
        pass

    class Both(Left, Right):
        pass

    classes = [Right, Both, Left, Both]
    leaf = (Both, (Left, Right))
    assert getclasstree(classes) == [
        (object, ()),
        [(Left, (object,)), [leaf], (Right, (object,)), [leaf]],
    ]
    assert getclasstree(classes, unique=True) == [
        (object, ()),
        [(Left, (object,)), [leaf], (Right, (object,))],
    ]


if __name__ == "__main__":
    current()
