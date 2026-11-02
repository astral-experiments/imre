import collections as containers
from collections import OrderedDict as Entries


WindowType = containers.deque


class Window(WindowType):
    pass


class Index(Entries):
    pass


index = Index(first=1, second=2)
index.move_to_end("first")
assert list(index.items()) == [("second", 2), ("first", 1)]

window = Window([1, 2, 3], maxlen=2)
window.append(4)
assert list(window) == [3, 4]
print(list(index), list(window))
