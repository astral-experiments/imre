import dataclasses
from dataclasses import Field, dataclass, field


@dataclass
class Model:
    names: list = field(default_factory=list)


descriptor = Model.__dataclass_fields__["names"]
assert Field is dataclasses.Field
assert type(descriptor) is Field
assert isinstance(field(default=7), Field)
assert descriptor.default_factory is list
assert Model().names is not Model().names
print(descriptor.name)
