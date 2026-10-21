# Source import order determines the initialization output being compared.
# ruff: noqa: I001
import left
import right
import package.child
from package import EXPORTED

print("main")
print(left.LEFT)
print(right.RIGHT)
print(package.child.VALUE)
print(EXPORTED)
