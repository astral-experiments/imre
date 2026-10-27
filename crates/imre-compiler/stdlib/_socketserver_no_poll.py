"""Checked TCP request dispatch remains available without a native poll backend."""


def _open_poll(fd: int | None) -> None:
    raise NotImplementedError("socketserver requires native poll support for serve_forever and shutdown")


def _poll_ready(selector: None, timeout: float | None) -> bool:
    raise NotImplementedError("socketserver requires native poll support")


def _close_poll(selector: None) -> None:
    raise NotImplementedError("socketserver requires native poll support")
