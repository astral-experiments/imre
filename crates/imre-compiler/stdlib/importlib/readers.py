"""Compatibility imports for resource readers."""

from .resources.readers import FileReader, MultiplexedPath, NamespaceReader, ZipReader

__all__ = ["FileReader", "ZipReader", "MultiplexedPath", "NamespaceReader"]
