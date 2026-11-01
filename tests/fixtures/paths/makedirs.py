from os import makedirs
from posixpath import split

makedirs("parent/leaf")
print(split("parent/leaf"))
