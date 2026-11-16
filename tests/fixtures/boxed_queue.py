import queue


def current():
    # A socket worker queues its exception for the main test to re-raise.
    errors = queue.Queue(1)
    error = ValueError("client failed")
    errors.put(error)
    print(errors.qsize(), errors.full())
    try:
        errors.put_nowait(None)
    except queue.Full:
        print("full")
    print(errors.get() is error, errors.empty())
    errors.task_done()
    errors.join()
    try:
        errors.task_done()
    except ValueError as exc:
        print(str(exc))

    for cls in (queue.Queue, queue.PriorityQueue, queue.LifoQueue, queue.SimpleQueue):
        items = cls()
        for item in (3, 1, 2):
            items.put_nowait(item)
        print([items.get_nowait() for _ in range(3)])
        try:
            items.get(timeout=0.001)
        except queue.Empty:
            print("empty")
        items.put("again")
        print(items.get())

    class Poll:
        calls = 0

        def __bool__(self):
            self.calls += 1
            return False

    poll = Poll()
    replies = queue.SimpleQueue()
    replies.put("ready")
    assert replies.get(poll, timeout=object()) == "ready"
    try:
        replies.get(poll, timeout=-1)
    except queue.Empty:
        pass
    else:
        raise AssertionError("empty poll returned an item")
    assert poll.calls == 2
