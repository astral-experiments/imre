import gc
import weakref
from collections.abc import ItemsView, KeysView


class Item:
    pass


class FreshItems:
    def __iter__(self):
        return self

    def __next__(self):
        return Item()

    def __getitem__(self, key):
        return Item()


mapping = FreshItems()
items = iter(ItemsView(mapping))
value = weakref.ref(next(items)[1])
keys = iter(KeysView(mapping))
key = weakref.ref(next(keys))
gc.collect()
print(value() is None, key() is None)
items.close()
keys.close()
