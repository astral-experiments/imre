from ._bootstrap import BuiltinImporter, FrozenImporter, ModuleSpec
from ._bootstrap_external import (
    BYTECODE_SUFFIXES,
    EXTENSION_SUFFIXES,
    SOURCE_SUFFIXES,
    FileFinder,
    NamespaceLoader,
    PathFinder,
    SourceFileLoader,
    SourcelessFileLoader,
)


def all_suffixes():
    """Return the recognized module filename suffixes for this process."""
    return SOURCE_SUFFIXES + BYTECODE_SUFFIXES + EXTENSION_SUFFIXES
