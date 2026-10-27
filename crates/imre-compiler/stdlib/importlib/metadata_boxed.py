from _imre_metadata import dedent, exists as _exists, filenames as _filenames, read_text as _read_text
from os.path import basename, isfile as _is_file, join as _join
import sys


class PackageNotFoundError(ModuleNotFoundError):
    def __str__(self):
        return f"No package metadata was found for {self.name}"

    @property
    def name(self):
        (name,) = self.args
        return name


def _paths():
    return sys.path


def _normalize_separators(name):
    return '_'.join(re.split(r'[-_.]+', name))


def _basename(root):
    return basename(root.rstrip('/'))
