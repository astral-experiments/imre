import sys
sys.path

visits = []

def collect(*arguments):
    visits.append(len(arguments))
    return arguments


def collect_tail(head, *tail):
    visits.append(head)
    return tail


has_fixed_parameter = len(sys.argv) > 1
function = collect_tail if has_fixed_parameter else collect
positions = tuple(range(114 if has_fixed_parameter else 113))
marker = {}
marker["probe"] = True
try:
    result = function(*positions)
except MemoryError as error:
    print("caught", error.args, len(visits))
print(len(function(*positions)), len(positions), len(visits))
