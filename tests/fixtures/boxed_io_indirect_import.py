from builtins import __import__ as load

module = load("".join(("i", "o")))
print(module.Reader[bytes].__origin__ is module.Reader, module.Writer.__module__)
