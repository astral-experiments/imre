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

    @contextmanager
    def stopping(self, stage: str, original: Exception, cleanup: Exception) -> None:
        if stage == "enter":
            raise original
        try:
            yield
        finally:
            if stage == "cleanup" or stage == "replace":
                raise cleanup
            if stage == "wrap":
                raise RuntimeError("wrapped") from original


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

    original = StopIteration("original")
    cleanup = StopIteration("cleanup")
    for stage in ["enter", "cleanup", "body", "replace", "wrap"]:
        try:
            with events.stopping(stage, original, cleanup):
                if stage != "cleanup":
                    raise original
        except RuntimeError as error:
            print(str(error))
            print(error.__cause__ is original)
            print(error.__cause__ is cleanup)
        except StopIteration as error:
            print(error is original)


run()
