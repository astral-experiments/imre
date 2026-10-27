import os
import os.path
import posixpath
from os import altsep, pathsep, sep
from os.path import altsep as path_altsep
from os.path import pathsep as path_list_sep
from os.path import sep as path_sep
from posixpath import pathsep as posix_pathsep


print(sep == os.sep)
print(path_sep == os.path.sep)
print(sep is path_sep)
print(altsep is path_altsep)
print(os.altsep == ("/" if os.name == "nt" else None))
print(os.sep == ("\\" if os.name == "nt" else "/"))
print(posixpath.sep == "/")
print(posixpath.altsep is None)
print(pathsep is path_list_sep)
print(os.pathsep is os.path.pathsep)
print(pathsep == (";" if os.name == "nt" else ":"))
print(posix_pathsep is posixpath.pathsep)
print((pathsep is posix_pathsep) == (os.name != "nt"))
print(posixpath.pathsep == ":")
print(("a" + pathsep + pathsep + "b").split(os.path.pathsep))
