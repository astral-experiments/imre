import builtins


class Spelling(str):
    pass


class Generated:
    def make_test(name):
        def test(self):
            return name
        return test

    for name in ("one", "two"):
        locals()["test_" + name] = make_test(name)
    first = test_one
    assert builtins.locals()["test_one"] is first

    locals()[Spelling("dynamic_name")] = test_one
    from_dynamic = dynamic_name
    locals()[Spelling("class_local")] = test_two
    from_local = class_local
    class_local = test_one


def current():
    case = Generated()
    assert Generated.first is Generated.test_one
    assert Generated.from_dynamic is Generated.test_one
    assert Generated.from_local is Generated.test_two
    assert Generated.class_local is Generated.test_one
    assert (case.test_one(), case.test_two()) == ("one", "two")
    assert not hasattr(case, "test_three")

    error = KeyError("comparison failed")

    class Key:
        def __hash__(self):
            return hash(name)

        def __eq__(self, other):
            raise error

    for operation, name in (
        ("read", "class_local"),
        ("read", "dynamic_name"),
        ("write", "class_local"),
    ):
        try:
            class Broken:
                locals()[Key()] = 1
                if operation == "write":
                    class_local = 2
                elif name == "class_local":
                    class_local = class_local
                else:
                    seen = dynamic_name
        except KeyError as caught:
            assert caught is error
        else:
            raise AssertionError("class creation swallowed the comparison error")


if __name__ == "__main__":
    current()
