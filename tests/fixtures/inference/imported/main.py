from package import compute
from package.worker import scale as alias
import package.worker as worker

print(compute([1, 2, 3]))
print(worker.scale(5, factor=3))
print(alias(6))
