from dataclasses import dataclass

from imre.json import decode


@dataclass
class Bounds:
    width: int = 0
    height: int = 0
    left: int = 0
    right: int = 0
    top: int = 0
    bottom: int = 0
    depth: int = 0
    scale: int = 0


bounds = decode('{"scale":2,"extra":17,"bottom":5,"width":7,"left":3}', Bounds)
print(bounds.width, bounds.left, bounds.bottom, bounds.scale, bounds.height)
try:
    decode('{"scale":1,"width":4,"scale":2}', Bounds)
except ValueError as error:
    print("scale" in str(error))
