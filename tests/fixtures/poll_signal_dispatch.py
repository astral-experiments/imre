import os
import select
import signal

idle, idle_writer = os.pipe()
ready, ready_writer = os.pipe()
poller = select.poll()
independent = select.poll()
os.write(ready_writer, b'x')
independent.register(ready, select.POLLIN)

def interrupt(signum, frame):
    poller.unregister(idle)
    poller.register(ready, select.POLLIN)
    try:
        poller.poll(0)
    except RuntimeError as error:
        assert str(error) == 'concurrent poll() invocation'
    else:
        raise AssertionError('same poller entered twice')
    assert independent.poll(0) == [(ready, select.POLLIN)]
    raise ValueError('interrupted')

def resume(signum, frame):
    poller.unregister(idle)
    poller.register(ready, select.POLLIN)
    os.write(idle_writer, b'x')

previous = signal.signal(signal.SIGALRM, interrupt)
try:
    poller.register(idle, select.POLLIN)
    signal.alarm(1)
    try:
        poller.poll(3000)
    except ValueError as error:
        assert str(error) == 'interrupted'
    else:
        raise AssertionError('lost interrupt')
    assert poller.poll(0) == [(ready, select.POLLIN)]

    poller.unregister(ready)
    poller.register(idle, select.POLLIN)
    signal.signal(signal.SIGALRM, resume)
    signal.alarm(1)
    assert poller.poll(3000) == [(idle, select.POLLIN)]
    assert poller.poll(0) == [(ready, select.POLLIN)]
    print('poll signal dispatch passed')
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, previous)
    for fd in (idle, idle_writer, ready, ready_writer):
        os.close(fd)
