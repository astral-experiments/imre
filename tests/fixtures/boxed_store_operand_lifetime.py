class Token:
    def __init__(self, name):
        self.name = name

    def __setitem__(self, key, value):
        print("set", key.name, value.name)
        return Token("ignored return")

    def __del__(self):
        print("drop", self.name)


class Manager:
    def __enter__(self):
        return 1, Token("active"), Token("sibling")

    def __exit__(self, kind, error, traceback):
        print("exit", kind.__name__)
        return True


def interrupted():
    with Manager() as (first, Token("receiver")[(yield "key")], last):
        raise AssertionError("target completed")
    print("after", first)


def current():
    first, *middle, Token("receiver")[Token("key")] = (1, 2, 3, Token("rhs"))
    print(first, middle)
    generator = interrupted()
    print(next(generator))
    generator.close()
