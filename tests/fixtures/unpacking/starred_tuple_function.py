def note(events: list[int], n: int) -> int:
    events.append(n)
    return n


def queue_shapes(events: list[int]) -> None:
    item = [7]
    queues = [item, item]
    factories = [note(events, n) for n in [1, 2]]
    result = (note(events, 3), *factories, *queues, note(events, 4))
    result[3].append(8)
    print(result[0])
    print(result[1])
    print(result[2])
    print(result[5])
    print(result[4][1])
    print(item[1])
    for event in events:
        print(event)


def fixed(pair: tuple[str, int]) -> tuple[str, int, int]:
    return (*pair, *[], *[19])


def fail() -> int:
    raise ValueError("field")


def ordered_error(events: list[int]) -> None:
    try:
        result = (note(events, 5), *[note(events, 6), fail()], note(events, 7))
        print(result[0])
    except ValueError:
        for event in events:
            print(event)


events: list[int] = []
queue_shapes(events)
values = fixed(("queue", 3))
print(values[0])
print(values[1])
print(values[2])
ordered_error(events)

module_fields = (*[1, 2],)
print(module_fields[1])
module_range = (note(events, 8), *range(3), note(events, 9))
print(len(module_range))
print(module_range[1])
print(events[-2])
print(events[-1])

unsafe = bytes((*range(0x21), 0x25, 0x7F)).decode()
print(len(unsafe))
print(ord(unsafe[-1]))
print(bytes((0, 255)) == b"\x00\xff")
print(bytes(()) == b"")
try:
    print(bytes((1, 1 << 65)))
except ValueError as error:
    print(str(error))
