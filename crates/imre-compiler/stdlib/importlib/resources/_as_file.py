"""Expose a Traversable as a local pathlib.Path for the duration of a context."""

import os
import tempfile
from _imre_tempfile import cleanup_directory
from contextlib import contextmanager

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


def _copy_entry(source, parent):
    name = source.name
    if not name or name in ('.', '..') or os.path.basename(name) != name:
        raise ValueError("expected a single resource name")
    child = parent / name
    if source.is_dir():
        child.mkdir()
        for entry in source.iterdir():
            _copy_entry(entry, child)
    else:
        with child.open('wb') as stream:
            stream.write(source.read_bytes())
    return child


@contextmanager
def _temp_directory(source, Path):
    assert source.is_dir()
    directory = tempfile.mkdtemp()
    try:
        yield _copy_entry(source, Path(directory))
    finally:
        cleanup_directory(directory)


@contextmanager
def _existing(path):
    yield path


def as_file(path):
    """Provide a local path, removing any temporary copy when the context exits."""
    from pathlib import Path

    if isinstance(path, Path):
        return _existing(path)
    try:
        directory = path.is_dir()
    except FileNotFoundError:
        directory = False
    if directory:
        return _temp_directory(path, Path)
    name = path.name
    if os.path.basename(name) != name:
        raise ValueError("expected a resource name without directory components")
    return _temp_file(path.read_bytes, name, Path)
