"""Declared native sibling-workspace subset with exception-safe cleanup."""

import json

from codon_platform import call


class TemporaryDirectory:
    name: str

    def __init__(self, dir, prefix: str):
        request = '{"directory":' + json.dumps(str(dir)) + ',"prefix":' + json.dumps(prefix) + '}'
        self.name = call(3, request)

    def __enter__(self) -> str:
        return self.name

    def __fspath__(self) -> str:
        # Codon binds the manager in `with ... as`; expose its owned directory.
        return self.name

    def __exit__(self):
        call(4, self.name)
