import ast
import sys

with open(sys.argv[1], "rb") as stream:
    tree = ast.parse(stream.read().decode())
count = 0
for node in ast.walk(tree):
    count += 1
print(count)
