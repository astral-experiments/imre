"""Calls supply the layouts of unannotated exception constructor fields."""


class Failure(Exception):
    def __init__(self, msg, entries):
        self.msg = msg
        self.entries = entries
        super().__init__(msg)

    def __str__(self):
        return self.msg


class Detail(Failure):
    def __init__(self, msg, entries, number):
        self.number = number
        super().__init__(msg, entries)


class Marker(Detail):
    pass


def run():
    entries = ["first"]
    exc = Marker("failed", entries, 37)
    try:
        raise exc
    except Detail as detail:
        print(detail is exc)
        print(detail.number)
        print(detail.entries is entries)
        entries.append("later")
        print(repr(detail.entries))
    try:
        raise exc
    except Failure as parent:
        print(parent is exc)
        print(str(parent))
        print(parent.entries is entries)


run()
