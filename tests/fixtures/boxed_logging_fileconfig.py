import io
import logging
import logging.config
import logging.handlers


class BespokeHandler(logging.Handler):
    def __init__(self, tag='default'):
        super().__init__()
        self.tag = tag

    def emit(self, record):
        pass


def configure(class_name, args='()', kwargs='{}'):
    config = f'''[loggers]
keys=root
[handlers]
keys=destination
[formatters]
keys=plain
[logger_root]
level=INFO
handlers=destination
[formatter_plain]
format=%(tag)s:%(message)s
defaults={{'tag': 'literal'}}
[handler_destination]
class={class_name}
formatter=plain
args={args}
kwargs={kwargs}
'''
    logging.config.fileConfig(io.StringIO(config))
    return logging.getLogger().handlers[0]


def current():
    logging.BespokeHandler = BespokeHandler
    try:
        for name in ('StreamHandler', 'logging.StreamHandler'):
            handler = configure(name)
            assert type(handler) is logging.StreamHandler
            assert handler.format(logging.makeLogRecord({'msg': 'hello'})) == 'literal:hello'
        for name in ('handlers.MemoryHandler', 'logging.handlers.MemoryHandler'):
            assert type(configure(name, '(1,)')) is logging.handlers.MemoryHandler
        assert configure('BespokeHandler', kwargs="{'tag': 'custom'}").tag == 'custom'
        assert configure(__name__ + '.BespokeHandler', "('qualified',)").tag == 'qualified'
    finally:
        del logging.BespokeHandler


def dynamic():
    try:
        configure('StreamHandler', '(sys.stderr,)')
    except ValueError as error:
        assert str(error) == (
            'handler_destination.args requires Python literal data; '
            'rewrite dynamic values using dictConfig'
        )
    else:
        raise AssertionError('accepted a dynamic fileConfig expression')
