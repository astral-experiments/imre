"""Compare the live dictionary behind a Python module's name bindings."""

from sys import path

first_bound = 1
second_bound = 2
first_bound = 3


def nested(closure_value):
    def inner():
        function_local = closure_value
        return globals(), function_local
    return inner


class WithNamespace:
    class_local = 17
    source_namespace = globals()


def update_declared():
    global first_bound, declared_value
    first_bound += 1
    declared_value = first_bound


def delete_declared():
    global declared_value
    del declared_value


def discard(assigned, child):
    def drop():
        nonlocal token
        del token

    if assigned:
        token = "local"
    if child:
        drop()
    else:
        del token


def current():
    for child in (False, True):
        try:
            discard(False, child)
        except NameError as error:
            print(type(error).__name__, str(error))
    module = __import__(__name__)
    mapping = globals()
    fetch = nested(23)
    captured, value = fetch()
    print(type(mapping) is dict, mapping is module.__dict__, mapping is current.__globals__)
    print(captured is mapping, fetch.__globals__ is mapping, WithNamespace.source_namespace is mapping)
    print(value, 'class_local' in mapping, 'closure_value' in mapping, 'function_local' in mapping)
    print([key for key in mapping if key in ('first_bound', 'second_bound')])
    print(mapping['first_bound'], module.first_bound)
    mapping['first_bound'] = 7
    print(first_bound, module.first_bound)
    module.first_bound = 11
    print(mapping['first_bound'], first_bound)
    update_declared()
    print(mapping['first_bound'], mapping['declared_value'], declared_value)
    delete_declared()
    print('declared_value' in mapping)
    values = mapping.values()
    mapping['fresh_value'] = 'added after making the view'
    print(fresh_value in values)
    del mapping['first_bound']
    try:
        print(first_bound)
    except NameError as error:
        print(str(error))
    mapping['first_bound'] = 1
    print([key for key in mapping if key in ('first_bound', 'second_bound')])
    for obj, name in ((current, '__globals__'), (module, '__dict__')):
        try:
            setattr(obj, name, {})
        except AttributeError as error:
            print(str(error))
    try:
        globals(1)
    except TypeError as error:
        print(str(error))
    try:
        globals(unexpected=True)
    except TypeError as error:
        print(str(error))

    class Finished:
        global marked_in_body
        marked_in_body = 'module value'

    Finished.marked_in_body = 'class value'
    print(marked_in_body, Finished.marked_in_body)
    del Finished.marked_in_body
    print(marked_in_body, hasattr(Finished, 'marked_in_body'))


if __name__ == '__main__':
    current()
