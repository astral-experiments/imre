import copy
import gc
import os
import pickle
import posix
import weakref


def current():
    current_param = os.sched_getparam(0)
    policy = os.sched_getscheduler(0)
    assert type(current_param) is os.sched_param
    assert current_param == posix.sched_getparam(os.getpid())
    for name in ("sched_getparam", "sched_setparam", "sched_setscheduler"):
        assert getattr(os, name) is vars(posix)[name]

    class PriorityIndex:
        def __index__(self):
            return current_param.sched_priority

    same_param = os.sched_param(PriorityIndex())
    # Reapply the existing policy and priority only. Some sandboxes deny either setter.
    for setter, args in ((os.sched_setscheduler, (0, policy, same_param)),
                         (os.sched_setparam, (0, same_param))):
        try:
            assert setter(*args) is None
        except PermissionError:
            pass
    assert os.sched_getscheduler(0) == policy and os.sched_getparam(0) == current_param

    for value, error in ((None, TypeError), (os.sched_param(None), TypeError),
                         (os.sched_param(1 << 40), OverflowError),
                         (os.sched_param(-(1 << 90)), OverflowError)):
        try:
            os.sched_setparam(0, value)
        except error:
            pass
        else:
            raise AssertionError("accepted invalid scheduling parameters")
    cls = posix.sched_param
    assert cls is os.sched_param and cls is posix.__dict__["sched_param"]
    assert type(cls) is type and (cls.__module__, cls.__name__) == ("posix", "sched_param")
    assert cls.__bases__ == (tuple,) and cls.__match_args__ == ("sched_priority",)
    assert (cls.n_fields, cls.n_sequence_fields, cls.n_unnamed_fields) == (1, 1, 0)
    field = cls.sched_priority
    assert field.__objclass__ is cls and field.__name__ == "sched_priority"

    payload = [7]
    p = cls(sched_priority=payload)
    assert isinstance(p, tuple) and type(p) is cls and p.__class__ is cls
    assert len(p) == 1 and p[-1] is payload and p[:] == (payload,)
    assert p.sched_priority is payload and field.__get__(p, cls) is payload
    assert repr(p) == "posix.sched_param(sched_priority=[7])"
    assert hash(cls(7)) == hash((7,)) and cls(7) == (7,)
    assert cls.__new__(cls, payload).sched_priority is payload
    assert cls(None).sched_priority is None and cls((1, 2)).sched_priority == (1, 2)

    for method in (p.__reduce__, lambda: cls.__reduce__(p)):
        factory, args = method()
        assert factory is cls and type(args) is tuple and len(args) == 1 and args[0] is payload
    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        factory, args = p.__reduce_ex__(protocol)
        assert factory is cls and args[0] is payload
        restored = pickle.loads(pickle.dumps(p, protocol))
        assert type(restored) is cls and restored is not p and restored.sched_priority == payload
    for same in (copy.copy(p), copy.replace(p), cls.__replace__(p)):
        assert type(same) is cls and same is not p and same.sched_priority is payload
    other = copy.deepcopy(p)
    assert type(other) is cls and other.sched_priority == payload and other.sched_priority is not payload
    assert copy.replace(p, sched_priority=None).sched_priority is None
    for function, args, kwargs in (
        (cls, (), {}), (cls, (1, 2), {}), (cls, (), {"priority": 1}),
        (cls, (1,), {"sched_priority": 2}), (cls.__new__, (tuple, 1), {}),
        (tuple.__new__, (cls, ()), {}), (tuple.__new__, (cls, (1,)), {}),
        (object.__new__, (cls,), {}), (cls.__reduce__, ((),), {}), (p.__reduce__, (0,), {}),
        (p.__reduce_ex__, ("4",), {}), (p.__replace__, (0,), {}),
        (field.__get__, ((), cls), {}),
    ):
        try:
            function(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted invalid arguments")
    try:
        p.__replace__(a=0, sched_priority=2, b=1)
    except TypeError as exc:
        assert str(exc) == "Got unexpected field name(s): ['a', 'b']"
    else:
        raise AssertionError("accepted unknown fields")
    try:
        p.sched_priority = 0
    except AttributeError:
        pass
    else:
        raise AssertionError("wrote read-only field")
    try:
        class Subclass(cls):
            pass
    except TypeError:
        pass
    else:
        raise AssertionError("subclassed sched_param")

    class Priority:
        pass

    value = Priority()
    ref = weakref.ref(value)
    record = cls(value)
    del value
    gc.collect()
    assert ref() is record.sched_priority
    reduction = record.__reduce__()
    del record
    gc.collect()
    assert ref() is reduction[1][0]
    del reduction
    gc.collect()
    assert ref() is None
    print("sched_param passed")
