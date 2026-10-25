from types import SimpleNamespace as Namespace
import types


def replace(source: dict[str, int]) -> int:
    source["first"] = 12
    source["during"] = 14
    return 15


def run() -> None:
    # test.support.run_in_subinterp_with_config constructs the namespace this way.
    config = {"gil": "own"}
    holder = types.SimpleNamespace(**config)
    config["gil"] = "changed"
    print(holder.gil)
    print(isinstance(holder, Namespace))
    print(isinstance(config, Namespace))
    print(isinstance(holder, dict))
    print(holder is not vars(holder))

    source = {"first": 1, "other": 2}
    ns = Namespace(source, other=replace(source), last=7)
    print(repr(ns))
    print(str(ns))
    print(ns)
    print(vars(ns) is ns.__dict__)
    attrs = ns.__dict__
    attrs["other"] = 27
    print(ns.other)
    ns.extra = 31
    print(attrs["extra"])
    del ns.other
    for _ in range(2):
        try:
            del ns.other
        except AttributeError as error:
            print(repr(error))
    ns.other = 40
    print(repr(ns))
    ns.__dict__.clear()
    try:
        print(ns.extra)
    except AttributeError as error:
        print(str(error))
    del ns
    print(attrs)
    print(repr(Namespace({"": 7, "x-y": 2, "é": 5})))

    items = ["one"]
    nested = Namespace(items=items)
    temporary = Namespace(**nested.__dict__).items
    print(temporary is items)
    exposed = vars(nested)
    del nested
    exposed["items"].append("two")
    print(items)
    temporary.append("three")
    print(exposed)

    first = Namespace(x=1)
    second = Namespace(x=1)
    print(first is second)
    print(first == second)
    second.x = 2
    print(first != second)
    print(bool(Namespace()))


run()
