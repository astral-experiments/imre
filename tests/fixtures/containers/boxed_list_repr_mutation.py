from dataclasses import dataclass

shown: list[object] = []

@dataclass(frozen=True)
class Ticket:
    number: int

    def __repr__(self) -> str:
        shown.clear()
        return "called"

tickets = {Ticket(1): "paid"}
try:
    tickets[Ticket(2)]
except KeyError as err:
    shown.append(err.args[0])
shown.append("removed")
print(shown)
print(len(shown))
