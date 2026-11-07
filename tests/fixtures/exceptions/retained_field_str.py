from dataclasses import dataclass

events: list[str] = []


@dataclass(frozen=True)
class Ticket:
    number: int

    def __str__(self) -> str:
        events.append("str")
        if self.number < 0:
            raise ValueError("invalid ticket")
        return "ticket " + str(self.number)

    def __repr__(self) -> str:
        events.append("repr")
        return "Ticket(" + str(self.number) + ")"


tickets = {Ticket(1): "paid"}
for query in (Ticket(2), Ticket(-1)):
    try:
        tickets[query]
    except KeyError as missing:
        try:
            print(str(missing.args[0]))
        except ValueError as error:
            print(str(error))
        print(str(missing.args))
        print(str(missing))
print(events)
