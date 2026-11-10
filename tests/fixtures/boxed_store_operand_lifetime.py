class Token:
    def __init__(self, name):
        self.name = name

    def __setitem__(self, key, value):
        print("set", key.name, value.name)
        return Token("ignored return")

    def __del__(self):
        print("drop", self.name)


def current():
    first, *middle, Token("receiver")[Token("key")] = (1, 2, 3, Token("rhs"))
    print(first, middle)
