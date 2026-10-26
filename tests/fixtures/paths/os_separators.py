import os
import os.path
import posixpath
from os import altsep, sep
from os.path import altsep as path_altsep
from os.path import sep as path_sep


print(sep == os.sep)
print(path_sep == os.path.sep)
print(sep is path_sep)
print(altsep is path_altsep)
print(os.altsep == ("/" if os.name == "nt" else None))
print(os.sep == ("\\" if os.name == "nt" else "/"))
print(posixpath.sep == "/")
print(posixpath.altsep is None)
