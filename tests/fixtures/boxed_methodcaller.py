from operator import methodcaller


class Receiver:
    def render(self, values, *, label):
        return label, values[:]


def replacement(self, values, *, label):
    return values[:], label


values = [1]
render = methodcaller("render", values, label="items")
receiver = Receiver()
print(render(receiver))
Receiver.render = replacement
values.append(2)
print(render(receiver))
