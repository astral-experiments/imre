import collections
from collections import namedtuple as make_type


def point():
    return make_type("Point", "x y")


def single(field):
    return collections.namedtuple(*("Single", [field]), **{"defaults": (None,)})


Pair = make_type("Pair", "x class x", rename=True)
Point = point()
Other = point()
print(Point is Other, Point(11, 22).y, single("name")().name)
print(repr(Point.x), repr(Point.y))
Point.y.__doc__ = "The y-coordinate"
print(repr(Point.y), repr(Other.y))

pair = Pair(1, 2, 3)
print(Pair._fields)
print(pair.x, pair._1, pair._2)
