# ruff: noqa: PLW0120
# Else suites intentionally test exhaustion even in loops with no local break.


for outer in range(3):
    for inner in range(2):
        print(outer + inner)
        break
    else:
        print("unreachable")
else:
    print("inner break does not suppress outer else")

for outer in range(3):
    for inner in range(0):
        print("unreachable")
    else:
        if outer == 0:
            continue
        break
    print("unreachable")
else:
    print("unreachable")
print("outer break from inner else")

count = 0
while count < 3:
    count += 1
    while False:
        print("unreachable")
    else:
        continue
    print("unreachable")
else:
    print(count)

for outer in range(3):
    try:
        for inner in range(0):
            print("unreachable")
        else:
            if outer == 1:
                break
            print(outer)
    finally:
        print("outer cleanup")
else:
    print("unreachable")

for outer in range(2):
    while False:
        print("unreachable")
    else:
        for inner in range(2):
            print(inner)
            break
else:
    print("break belongs to loop inside else")
