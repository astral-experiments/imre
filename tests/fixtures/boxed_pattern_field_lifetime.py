class Field:
    def __init__(self, name):
        self.name = name

    @property
    def label(self):
        print("read", self.name)
        return self.name

    @property
    def child(self):
        return Field(self.name + " child")

    def __del__(self):
        print("drop", self.name)


class Pair:
    def __getattr__(self, name):
        print("get", name)
        return Field(name)


def current():
    match Pair():
        case Pair(left=Field(label=left), right=Field(label=right)):
            print(left, right)
    match Pair():
        case Pair(left=_, right=Field(label=right)):
            print(right)
    match Pair():
        case (Pair(left=Field(child=saved), right=Field(label="absent"))
              | Pair(left=Field(child=saved), right=Field(label="right"))):
            print("bound", saved.name)
