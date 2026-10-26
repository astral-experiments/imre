"""Hold one POSIX selector for a TCP server loop or an empty shutdown wait."""
from selectors import EVENT_READ, PollSelector


def _open_poll(fd: int | None) -> PollSelector:
    selector = PollSelector()
    if fd is not None:
        try:
            selector.register(fd, EVENT_READ)
        except:
            selector.close()
            raise
    return selector


def _poll_ready(selector: PollSelector, timeout: float | None) -> bool:
    return bool(selector.select(timeout))


def _close_poll(selector: PollSelector) -> None:
    selector.close()
