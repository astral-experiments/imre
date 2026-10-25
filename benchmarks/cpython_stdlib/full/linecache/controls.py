"""Exercise live cache aliases before/after mutation; the caller supplies file bytes."""

import linecache
import sys

path = sys.argv[1]
extra = sys.argv[2]
linecache.clearcache()
cache = linecache.cache
print(cache is linecache.cache)
first = linecache.getlines(path)
print(first)
print(first is linecache.getlines(filename=path))
print(first is cache[path][2])
print(linecache.getline(path, 1) is first[0])
first[0] = "changed"
first.append("tail")
print(linecache.getline(path, lineno=1))
print(linecache.getline(path, len(first)))
saved = cache[path]
print(saved[0])
print(saved[1] > 0.0)
print(saved[3] == path)
print(saved[3] is path)
with open(path, "w", encoding="utf-8") as file:
    file.write("replacement")
linecache.checkcache("unrelated")
print(linecache.getlines(path) is first)
linecache.checkcache(filename=path)
print(path in cache)
print(first)
second = linecache.getlines(path)
print(second)
linecache.clearcache()
print(len(cache))
print(second)
print(linecache.getlines(path) is second)
forced = linecache.updatecache(path)
print(forced is linecache.getlines(path))
print(forced is second)
del cache[path]
print(path in linecache.cache)
print(forced)
print(linecache.getline(path, 10**100))
print(linecache.getline(path, True))
print(linecache.getline(path, float("nan")))
print(linecache.getline(path, -8.0))
try:
    linecache.getline(path, 1.0)
except TypeError as error:
    print("TypeError")
    print(str(error))
print(linecache.getlines("<string>"))
print(linecache.getlines("<frozen nowhere>"))
print(linecache.getlines("<frozen source>", {"__file__": path}))
print(linecache.getlines("<frozen source>") is linecache.cache["<frozen source>"][2])
print(linecache.cache["<frozen source>"][3] is path)
linecache.cache["virtual"] = (0, 1.0, ["injected"], extra)
print(linecache.getlines("virtual") is linecache.cache["virtual"][2])
linecache.checkcache("virtual")
print("virtual" in cache)
print(linecache.getlines(extra))
print(extra in cache)
print(linecache.getlines(extra) is linecache.getlines(extra))
print(linecache.lazycache(path, None))

nothing = ""
newline = "\n"
empty_source = sys.argv[3]
two_blank = sys.argv[4]
miss = linecache.getline(extra, 1)
print(miss is nothing)
print(miss is linecache.getline(empty_source, 4))
print(miss is linecache.getline(extra, 2.0))
print(linecache.getline(empty_source, 1) is newline)
print(linecache.getline(two_blank, 1) is newline)
print(linecache.getline(two_blank, 1) is linecache.getline(two_blank, 2))
