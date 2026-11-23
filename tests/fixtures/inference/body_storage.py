def factory(n):
    def add(x):
        return n + x

    n += 1
    return add


def show(x):
    return x


class A:
    def show(self, x):
        return x


def choose(flag):
    return 1 if flag else "one"


def either(flag):
    if flag:
        return 1
    return "one"


def change(flag):
    value = 2
    if flag:
        value = "two"
    return value


left, right = factory(2), factory(10)
print(left(3), right(3), left(3))
print(show(1), show("a"))
a = A()
print(a.show("a"), a.show(1))
print(choose(False), choose(True), change(False), change(True))
print(either(False), either(True))


def default(x=1):
    return x


def store(x):
    global current
    x = "updated"
    current = x
    return x


def loop(text):
    item = 1
    for item in text:
        print(item)
    return item


current = 1
print(current, store(1), current)
print(default(), default("other"), default())
print(loop(""), loop("s"))
x = 1
pair = ("first", 2)
x, y = pair
print(x, y, x := 3, x)
same, same = (1, "two")
ratio = 1
ratio /= 2
print(same, ratio)
