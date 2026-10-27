import importlib.metadata
from importlib import metadata as distributions
from importlib.metadata import PackageNotFoundError, version

from consumer import server_version


for name in ["Field.Notes", "FIELD__NOTES", "field---notes"]:
    assert importlib.metadata.version(name) == "2.1rc3+local"
    assert distributions.version(name) == version(name)
    print(server_version(name))

try:
    version("absent-field-notes")
except PackageNotFoundError as error:
    print(str(error))
    print(repr(error))
    print(error.args)
    print(error.name)
    assert isinstance(error, ModuleNotFoundError)
    assert isinstance(error, ImportError)
else:
    raise AssertionError("missing distribution")
