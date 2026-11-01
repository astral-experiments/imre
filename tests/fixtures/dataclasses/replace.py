from dataclasses import dataclass, field, replace


@dataclass(frozen=True)
class Model:
    name: str
    values: list = field(default_factory=list)


original = Model('draft')
updated = replace(original, name='published')
assert original.name == 'draft'
assert updated.name == 'published'
assert updated.values is original.values
