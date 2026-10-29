import sys

with open(sys.argv[1]) as stream:
    code = compile(stream.read(), "nested.py", "exec")
namespace = {}
exec(code, namespace)
print(namespace["result"])
