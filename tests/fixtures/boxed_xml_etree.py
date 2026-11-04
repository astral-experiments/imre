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
