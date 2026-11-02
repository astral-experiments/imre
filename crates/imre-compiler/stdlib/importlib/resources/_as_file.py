"""Expose a Traversable as a local pathlib.Path for the duration of a context."""

import os
import tempfile
from contextlib import contextmanager

from importlib.resources._filesystem import _FilePath


def _remove(path, remove=os.remove):
    try:
        remove(path)
    except FileNotFoundError:
        pass


@contextmanager
def _temp_file(reader, name, Path):
    fd, filename = tempfile.mkstemp(suffix=name)
    try:
        try:
            with open(fd, 'wb', closefd=False) as stream:
                stream.write(reader())
        finally:
            os.close(fd)
        del reader
        yield Path(filename)
    finally:
        _remove(filename)


@contextmanager
def _existing(path):
    yield path


def as_file(path):
    """Provide a local path, removing any temporary copy when the context exits."""
    from pathlib import Path

    if isinstance(path, Path):
        return _existing(path)
    if isinstance(path, _FilePath):
        return _existing(Path(path))
    try:
        directory = path.is_dir()
    except FileNotFoundError:
        directory = False
    if directory:
        raise NotImplementedError("materializing virtual resource directories is unsupported")
    name = path.name
    if os.path.basename(name) != name:
        raise ValueError("expected a resource name without directory components")
    return _temp_file(path.read_bytes, name, Path)
