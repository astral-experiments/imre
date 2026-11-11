def maker():
    def keep[T](value=(yield 'default')):
        return value
    return keep


token = []
it = maker()
print(next(it))
try:
    it.send(token)
except StopIteration as stop:
    keep = stop.value
    print(keep() is token, keep.__defaults__[0] is token, keep.__type_params__[0].__name__)
