from typing import Protocol, TypeVar, get_args

T = TypeVar("T")
U = TypeVar("U")

class Packet(Protocol[*[T], U]):
    pass

parameters = get_args(Packet.__orig_bases__[0])
assert parameters[0] is T
assert parameters[1] is U
print("protocol", len(parameters))
