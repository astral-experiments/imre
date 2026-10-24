from contextlib import contextmanager
from dataclasses import dataclass, field


@dataclass
class Events:
    events: list[str] = field(default_factory=list)

    @contextmanager
    def opened(self, label: str, noop: bool) -> None:
        if noop:
            yield
            return
        self.events.append(label)
        try:
            yield
        finally:
            self.events.append("close " + label)


def run() -> None:
    events = Events()
    try:
        with events.opened("a", False), events.opened("unused", True):
            with events.opened("b", False):
                events.events.append("body")
                raise ValueError("failed")
    except ValueError as error:
        print(str(error))
    print("|".join(events.events))


run()
