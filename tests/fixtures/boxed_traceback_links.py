import gc
import weakref


class Local:
    pass


def fail(refs):
    held = Local()
    refs.append(weakref.ref(held))
    raise LookupError()


def capture(refs):
    try:
        fail(refs)
    except LookupError as error:
        return error


def check():
    refs = []
    original = capture(refs)
    head = original.__traceback__
    tail = head.tb_next
    alias = ValueError().with_traceback(head)
    assert refs[0]() is not None

    for target, value, error_type, detail in (
        (head, 42, TypeError, "expected traceback object, got 'int'"),
        (head, head, ValueError, "traceback loop detected"),
        (tail, head, ValueError, "traceback loop detected"),
    ):
        try:
            target.tb_next = value
        except error_type as error:
            assert str(error) == detail
        else:
            raise AssertionError("invalid link accepted")
    del target, value
    try:
        del head.tb_next
    except TypeError as error:
        assert str(error) == "can't delete tb_next attribute"
    else:
        raise AssertionError("link deleted")
    assert original.__traceback__ is alias.__traceback__ is head
    assert head.tb_next is tail and tail.tb_next is None

    replacement = capture([]).__traceback__.tb_next
    head.tb_next = replacement
    del tail
    gc.collect()
    assert refs[0]() is None
    assert alias.__traceback__.tb_next is original.__traceback__.tb_next is replacement

    head.tb_next = None
    try:
        raise alias
    except ValueError as caught:
        assert caught is alias and caught.__traceback__.tb_next is head
        assert head.tb_next is None


check()
