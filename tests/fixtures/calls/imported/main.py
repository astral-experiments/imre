import helpers as module
from facade import ImportedItem as Item
from facade import imported_label as label
from relay import bridge


def relay(number: int, **options) -> str:
    return label(number, **options)


print(relay(40))
print(relay(41, text="forwarded"))
print(bridge())
print(bridge(shift=3, mark="overridden"))
print(label())
print(label(text="aliased", number=11))
print(module.label(number=12, text="module"))
print(Item().label())
print(Item(number=13).label(prefix="constructor"))
for value in [label(text="comprehension", number=index) for index in range(3)]:
    print(value)
