check = "ⅷ!".islower
print(check())
try:
    check(0)
except TypeError as error:
    print(error)
