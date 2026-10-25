"""Observe the root file handler and when configuration validates its options."""
import io
import logging
from pathlib import Path


def value(label: str) -> str:
    print(label)
    return label


def fail_option() -> str:
    raise RuntimeError("option evaluated before reconfiguration")


def exercise() -> None:
    root = logging.getLogger("")
    live = root.handlers
    path = Path("file.log")
    logging.basicConfig(filename=path, filemode="w", encoding="utf-8", errors="strict",
                        level="DEBUG", format="%(message)s")
    print(len(live))
    file_handler = live[0]
    print(isinstance(file_handler, logging.FileHandler))
    logging.debug("The Øresund Bridge joins Copenhagen to Malmö")
    print(path.read_bytes())
    logging.basicConfig(extra=value("ignored option was evaluated"), filename=path, stream=None)
    print(len(live))
    try:
        logging.basicConfig(force=True, extra=fail_option(), filename=path)
    except RuntimeError as exc:
        print(str(exc))
    print(live[0] is file_handler)
    print(file_handler._closed)
    try:
        logging.basicConfig(force=True, filename=None, stream=None)
    except ValueError as exc:
        print(str(exc))
    print(file_handler._closed)
    print(len(live))
    try:
        logging.basicConfig(filename=path, handlers=[])
    except ValueError as exc:
        print(str(exc))
    print(len(live))
    logging.basicConfig(filename=None, filemode="a")
    print(isinstance(live[0], logging.StreamHandler))
    print(isinstance(live[0], logging.FileHandler))
    logging.basicConfig(force=True, filename=path, format="%(message)s")
    logging.error("appended")
    print(path.read_bytes())
    logging.basicConfig(force=True, filename="", filemode="w")
    print(isinstance(live[0], logging.FileHandler))
    try:
        logging.basicConfig(force=True, filename=Path("."), filemode="w")
    except IsADirectoryError as exc:
        number = exc.errno
        assert number is not None
        print(number)
    print(len(live))
    path.unlink()
    stream = io.StringIO()
    replacement = logging.StreamHandler(stream)
    try:
        logging.basicConfig(force=True, handlers=[replacement], second=value("second"),
                            filemode=value("filemode"), first=value("first"), level="ERROR")
    except ValueError as exc:
        print(str(exc))
    print(root.level)
    print(live[0] is replacement)
    logging.error("installed before error")
    print(repr(stream.getvalue()))
    try:
        logging.basicConfig(force=True, loglevel=value("unknown"))
    except ValueError as exc:
        print(str(exc))
    print(len(live))
    print(replacement._closed)


exercise()
