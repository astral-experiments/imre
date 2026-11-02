from types import DynamicClassAttribute


def current():
    class Meta(type):
        def __getattr__(cls, name):
            if name == "label":
                return cls.__name__
            raise AttributeError(name)

    class Field(metaclass=Meta):
        @DynamicClassAttribute
        def label(self):
            """Instance label."""
            return self._label

        @label.setter
        def label(self, value):
            self._label = value

    assert Field.label == "Field"
    field = Field()
    field.label = "value"
    assert field.label == "value"
    assert vars(Field)["label"].__doc__ == "Instance label."
