import io
import logging.handlers
import logging.handlers as handlers
import queue
from logging.handlers import QueueHandler as Producer, QueueListener as Listener

assert Producer is handlers.QueueHandler is logging.handlers.QueueHandler
assert Listener is handlers.QueueListener is logging.handlers.QueueListener
items = queue.Queue()
producer = Producer(items)
stream = io.StringIO()
sink = logging.StreamHandler(stream)
sink.setLevel(logging.WARNING)
listener = Listener(items, sink, respect_handler_level=True)
assert producer.queue is listener.queue is items

for level, message in ((logging.DEBUG, 'debug'), (logging.WARNING, 'warning')):
    record = logging.LogRecord('probe', level, 'example.py', 1, 'item %s', (message,), None)
    producer.handle(record)
    queued = listener.dequeue(False)
    assert queued is not record and record.msg == 'item %s' and record.args == (message,)
    assert queued.msg == queued.message == 'item ' + message and queued.args is None
    listener.handle(queued)
    items.task_done()

listener.enqueue_sentinel()
assert listener.dequeue(False) is None
items.task_done()
items.join()
assert stream.getvalue() == 'item warning\n'
print(stream.getvalue(), end='')
