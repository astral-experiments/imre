import tomllib

with open("project.toml", "wb") as stream:
    stream.write(b'[project]\nname = "demo"\nversions = [1, 2]\n')
with open("project.toml", "rb") as stream:
    document = tomllib.load(stream)

print(document["project"])
keys = document.keys()
print(repr(keys), keys == {"project"})

try:
    keys.add("new")
except AttributeError:
    print("keys view is read-only")

try:
    document.keys("project")
except TypeError:
    print("keys takes no arguments")

with open("project.toml", encoding="utf-8") as stream:
    try:
        tomllib.load(stream)
    except TypeError:
        print("load requires a binary stream")

try:
    tomllib.loads(b"x = 1")
except TypeError as error:
    print(error)
