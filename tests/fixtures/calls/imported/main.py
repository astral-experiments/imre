import helpers as module
from facade import ImportedItem as Item
from facade import imported_label as label

print(label())
print(label(text="aliased", number=11))
print(module.label(number=12, text="module"))
print(Item().label())
print(Item(number=13).label(prefix="constructor"))
for value in [label(text="comprehension", number=index) for index in range(3)]:
    print(value)
