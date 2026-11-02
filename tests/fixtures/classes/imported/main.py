import models as package
import models.records as definitions
from models.records import Counter as Original
from models import ExportedCounter as Reexport
from facade import Count as FacadeCounter
from facade import passthrough


class ExplicitCounter(Original):
    def __init__(self, super: int) -> None:
        definitions.Counter.__init__(self, total=super)


def use_qualified(value: "definitions.Counter") -> "package.ExportedCounter":
    return value


def use_reexport(value: Reexport) -> Original:
    return value


def main() -> None:
    first = Original(1000000000000000000000000000000)
    second: package.ExportedCounter = first
    third: FacadeCounter = use_qualified(use_reexport(second))
    print(passthrough(third).bump(7))
    print(first.total)
    print(package.create(17).total)
    print(Reexport(4).bump(2))
    print(definitions.Counter(5).total)
    print(ExplicitCounter(11).total)


main()
