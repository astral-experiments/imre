from collections import OrderedDict as Table
from collections import deque as Window


def make():
    class LocalMap(Table):
        pass

    class LocalWindow(Window):
        pass

    return LocalMap(x=1), LocalWindow([1, 2], maxlen=2)


mapping, series = make()
mapping["y"] = 2
mapping.move_to_end("x")
series.append(3)
print(list(mapping.items()), list(series))
