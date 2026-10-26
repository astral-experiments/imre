"""Binary update streams for the POSIX tempfile.SpooledTemporaryFile export."""

from io import BytesIO
from tempfile import TemporaryFile
from typing import BinaryIO


def _binary_mode(mode: str) -> str:
    if mode not in ("rb+", "r+b", "wb+", "w+b"):
        raise NotImplementedError("SpooledTemporaryFile requires a binary update mode")
    return mode


class SpooledTemporaryFile:
    _file: BinaryIO
    _rolled: bool
    _max_size: int
    _mode: str
    _dir: str | None
    _name: int | None

    def __init__(self, max_size: int = 0, mode: str = "w+b", *, dir: str | None = None) -> None:
        self._file = BytesIO()
        self._rolled = False
        self._max_size = max_size
        self._mode = _binary_mode(mode)
        self._dir = dir
        self._name = None

    def rollover(self) -> None:
        if self._rolled:
            return
        memory = self._file
        backing = TemporaryFile(dir=self._dir)
        self._file = backing
        position = memory.tell()
        assert isinstance(memory, BytesIO)
        backing.write(memory.getvalue())
        backing.seek(position)
        self._name = backing.fileno()
        self._rolled = True

    def write(self, data: bytes) -> int:
        size = self._file.write(data)
        if not self._rolled and self._max_size and self._file.tell() > self._max_size:
            self.rollover()
        return size

    def read(self, size: int = -1) -> bytes:
        return self._file.read(size)

    def readline(self) -> bytes:
        return self._file.readline()

    def seek(self, offset: int, whence: int = 0) -> int:
        return self._file.seek(offset, whence)

    def tell(self) -> int:
        return self._file.tell()

    def fileno(self) -> int:
        self.rollover()
        return self._file.fileno()

    def flush(self) -> None:
        self._file.flush()

    def close(self) -> None:
        self._file.close()

    @property
    def closed(self) -> bool:
        return self._file.closed

    @property
    def name(self) -> int | None:
        return self._name

    @property
    def mode(self) -> str:
        return "rb+" if self._rolled else self._mode

    def __enter__(self) -> "SpooledTemporaryFile":
        if self.closed:
            raise ValueError("Cannot enter context with closed file")
        return self

    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> None:
        self.close()
