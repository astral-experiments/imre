from types import GenericAlias


class Batch(ExceptionGroup):
    pass


class CustomBatch(Batch):
    def __class_getitem__(cls, item):
        return cls, item


class DelegatingBatch(Batch):
    def __class_getitem__(cls, item):
        return super().__class_getitem__(item)


class ConcreteBatch(Batch):
    __class_getitem__ = None


def handle(errors: ExceptionGroup[OSError]) -> BaseExceptionGroup[ValueError]:
    return errors


def current():
    for group in (BaseExceptionGroup, ExceptionGroup, Batch, DelegatingBatch):
        alias = group[OSError]
        assert type(alias) is GenericAlias
        assert alias.__origin__ is group and alias.__args__ == (OSError,)
        assert group.__class_getitem__(OSError) == alias
    assert CustomBatch[OSError] == (CustomBatch, OSError)
    assert handle.__annotations__ == {
        "errors": ExceptionGroup[OSError],
        "return": BaseExceptionGroup[ValueError],
    }
    for cls in (Exception, ConcreteBatch):
        try:
            cls[OSError]
        except TypeError as exc:
            assert str(exc) == f"type '{cls.__name__}' is not subscriptable"
        else:
            raise AssertionError(f"{cls} accepted a type argument")
