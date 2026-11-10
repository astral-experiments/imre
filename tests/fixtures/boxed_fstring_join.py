class Part(str):
    def __del__(self):
        print("drop", self)

class Spelling(str):
    def __format__(self, spec):
        return Part(self)

class Value:
    def __init__(self, label):
        self.label = label

    def __repr__(self):
        return Spelling(self.label)

def two():
    print(f"<{Value('a')!r}{Value('b')!r}>")

def three():
    print(f"<{Value('a')!r}{Value('b')!r}{Value('c')!r}>")

def four():
    print(f"<{Value('a')!r}{Value('b')!r}{Value('c')!r}{Value('d')!r}>")

def current():
    two()
    three()
    four()
    print(f"values: {', '.join(repr(value) for value in (1, 2))}")
