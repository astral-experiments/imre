def recurse(n):
    if n:
        return 1 + recurse(n - 1)
    return 0

def endless():
    return endless()

def current():
    print(recurse(32), recurse(256))
    for _ in range(2):
        try:
            endless()
        except RecursionError:
            print("recursion caught")
        print(recurse(32))
