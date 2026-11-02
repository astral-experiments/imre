"""Compatibility imports for resource readers."""

from .resources.readers import MultiplexedPath, NamespaceReader, ZipReader

__all__ = ["ZipReader", "MultiplexedPath", "NamespaceReader"]
