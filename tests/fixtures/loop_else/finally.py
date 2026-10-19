# ruff: noqa: B012, PLW0120
# These transfers intentionally replace pending completion in finally suites.


number = -1
for number in range(3):
    try:
        print(number)
        break
    finally:
        continue
else:
    print("break replaced by continue")
for number in range(3):
    try:
        continue
    finally:
        break
else:
    print("unreachable")
print(number)

count = 0
while count < 3:
    count += 1
    try:
        break
    finally:
        continue
else:
    print(count)
count = 0
while count < 3:
    count += 1
    try:
        continue
    finally:
        break
else:
    print("unreachable")
print(count)


def suppressed_return() -> str:
    for number in range(2):
        try:
            return "discarded " + str(number)
        finally:
            continue
    else:
        return "return replaced by continue"


def overriding_return() -> str:
    for number in range(2):
        try:
            break
        finally:
            return "break replaced by return"
    else:
        return "empty"


print(suppressed_return())
print(overriding_return())
for number in range(2):
    try:
        raise ValueError("suppressed")
    finally:
        continue
else:
    print("exception replaced by continue")
try:
    for number in range(2):
        try:
            break
        finally:
            raise ValueError("break replaced by exception")
    else:
        print("unreachable")
except ValueError as error:
    print(error)
