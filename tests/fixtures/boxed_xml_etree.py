import gc
import weakref
import xml.etree.ElementTree as ET
from xml.etree.ElementTree import Element
import xml

assert xml.etree.ElementTree is ET
assert ET.Element is Element

root = Element("root")
child = Element("{urn:demo}item", {"id": "7"})
root.append(child)
assert root.find("p:item", {"p": "urn:demo"}) is child
assert child.get("id") == "7"
print(child.tag, child.attrib["id"])

# The Python implementation delegates to custom child iterators.
base = ET._Element_Py
branch = base("branch")
leaf = base("leaf")
branch.append(leaf)
nodes = branch.iter("*")
assert next(nodes) is branch
assert next(nodes) is leaf
added = base("added")
leaf.append(added)
assert list(nodes) == [added]

temporary_visits = []


class CustomChild(base):
    @property
    def iter(self):
        def visit(tag):
            assert tag == "custom"
            return iter([self])

        temporary_visits.append(weakref.ref(visit))
        return visit


custom = CustomChild("custom child")
branch.insert(0, custom)
nodes = branch.iter("custom")
assert next(nodes) is custom
gc.collect()
assert temporary_visits[0]() is None
assert list(nodes) == []

leaf.iter = lambda tag: iter([added])
assert list(branch.iter("custom")) == [custom, added]

# An ordinary bound method can redirect iteration to a different element.
redirected = base("root")
proxy = base("target")
target = base("target")
redirected.append(proxy)
proxy.iter = target.iter
assert list(redirected.iter("target")) == [target]
