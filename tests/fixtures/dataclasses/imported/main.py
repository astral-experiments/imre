from facade import Entry
from decorators import record
from models import Stamp as ModelStamp, erased, make_stamp


@record(slots=True)
class Stamp:
    value: int


def inspect(stamp: object, original: ModelStamp) -> None:
    print(isinstance(stamp, ModelStamp))
    print(isinstance(stamp, Stamp))
    print(stamp is original)
    print(stamp == original)
    print(stamp == Stamp(3))
    if isinstance(stamp, ModelStamp):
        print(stamp.value)


row = Entry.named("aliased")
row.values.append(4)
print(row.name)
print(row.count)
print(row)

local = ModelStamp(3)
inspect(erased(local), local)
inspect(make_stamp(3), local)
inspect(Stamp(3), local)
