import sys

from _frozen_importlib import ModuleSpec
from _frozen_importlib_external import SourceFileLoader
from importlib.machinery import EXTENSION_SUFFIXES

SOURCE_SUFFIXES = [".py", ".pyw"] if sys.platform == "win32" else [".py"]
BYTECODE_SUFFIXES = [".pyc"]


def all_suffixes():
    """Return the recognized module filename suffixes for this process."""
    return SOURCE_SUFFIXES + BYTECODE_SUFFIXES + EXTENSION_SUFFIXES
