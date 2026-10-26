from contextvars import Context

events: list[int] = []


def receiver() -> Context:
    events.append(1)
    return Context()


def wrong() -> int:
    events.append(2)
    return 37


def eager() -> int:
    events.append(3)
    return 0


try:
    receiver()[wrong()]
except TypeError as exc:
    print(str(exc))
print(events)
events.clear()
try:
    wrong() in receiver()
except TypeError as exc:
    print(str(exc))
print(events)
events.clear()
try:
    receiver().get(wrong(), eager())
except TypeError as exc:
    print(str(exc))
print(events)
events.clear()
try:
    receiver().run(x=eager())
except TypeError as exc:
    print(str(exc))
print(events)
