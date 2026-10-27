import sys
sys.path

visits = []

def collect(*arguments):
    visits.append(len(arguments))
    return arguments


positions = tuple(range(113))
marker = {}
marker["probe"] = True
try:
    result = collect(*positions)
except MemoryError as error:
    print("caught", error.args, len(visits))
print(len(collect(*positions)), len(positions), len(visits))
