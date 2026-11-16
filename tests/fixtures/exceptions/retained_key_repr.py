from dataclasses import dataclass

events: list[str] = []
labels = ["first"]


@dataclass(frozen=True)
class Ticket:
    number: int

    def __repr__(self) -> str:
        events.append(labels[0])
        if self.number < 0:
            raise ValueError("invalid ticket")
        return labels[0] + "-" + str(self.number)


invoices = {Ticket(1): "paid"}
try:
    invoices[Ticket(2)]
except KeyError as missing:
    print(len(events))
    print(str(missing))
    labels[0] = "second"
    print(str(missing))

try:
    invoices[Ticket(-1)]
except KeyError as missing:
    try:
        print(str(missing))
    except ValueError as broken:
        print(str(broken))

print(events)
