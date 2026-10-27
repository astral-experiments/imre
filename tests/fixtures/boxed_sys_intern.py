def current():
    compiled_source_pool()
    module_binding_keys()
    import sys
    from sys import intern

    assert intern is sys.intern
    assert sys._is_interned("jinja_token")
    letter = "é"
    assert intern(chr(233)) is letter

    fresh = "".join(["jinja_future_", "identifier"])
    known = intern(fresh)
    assert known is not fresh
    literal = "jinja_future_identifier"
    assert known is literal

    def word(n):
        return "live spelling " + str(n)

    first, second = word(721), word(721)
    assert first == second and first is not second
    assert not sys._is_interned(first)
    assert intern(first) is first and sys._is_interned(first)
    assert intern(second) is first and not sys._is_interned(second)
    aliases = [first]
    del first
    assert intern(second) is aliases[0]
    del aliases
    assert intern(second) is second and sys._is_interned(second)

    λglobal = 0
    literal = "λglobal"
    assert not sys._is_interned(literal)

    def unused(πunused, *, φkeyword=None, **δextras):
        return λglobal.ηattribute

    for prefix, suffix in (("λ", "global"), ("π", "unused"), ("φ", "keyword"),
                           ("δ", "extras"), ("η", "attribute")):
        fresh = "".join((prefix, suffix))
        assert intern(fresh) is not fresh
    fresh = "".join(("ψ", "literal"))
    assert not sys._is_interned("ψliteral") and intern(fresh) is fresh

    class Name(str):
        def __hash__(self):
            raise AssertionError("called subclass __hash__")

    subclass = Name("jinja_token")
    assert not sys._is_interned(subclass)
    for call in (lambda: intern(subclass), lambda: intern(b"word"), lambda: intern(None),
                 lambda: intern(), lambda: intern("one", "two"), lambda: intern(value="x")):
        try:
            call()
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("accepted invalid sys.intern arguments")
    print("released canonical string and adopted a live equal string")


def compiled_source_pool():
    import sys

    name = ''.join(('runtime_ξ_', '1123'))
    word = ''.join(('runtime_pool_', '1137'))
    code = compile(name + " = '" + word + "'", '<pool>', 'exec')
    print(sys.intern(name) is name, sys.intern(word) is word)
    scope = {}
    exec(code, scope)
    print(sys._is_interned(scope[name]), sys.intern(word) is scope[name])
    del scope, code
    print(sys.intern(name) is name, sys.intern(word) is word)

    keyword = ''.join(('runtime_λ_', '1171'))
    code = compile('target(' + keyword + '=1)', '<keyword>', 'exec')
    print(sys.intern(keyword) is keyword)


def module_binding_keys():
    import sys

    def key_from(mapping, text):
        for key in mapping:
            if key == text:
                return key
        raise KeyError(text)

    global λbinding_after_materialization
    namespace = globals()
    spelling = "".join(("module_binding_", "keys"))
    key = key_from(namespace, spelling)
    print(sys._is_interned(key), sys.intern(spelling) is key)

    λbinding_after_materialization = 17
    spelling = "".join(("λbinding_", "after_materialization"))
    key = key_from(namespace, spelling)
    print(sys._is_interned(key), sys.intern(spelling) is key, namespace[key])

    original = "".join(("external_binding_", "identity"))
    supplied = {original: 1}
    exec("external_binding_identity = 19", supplied)
    key = key_from(supplied, "".join(("external_binding_", "identity")))
    print(key is original, sys._is_interned(key), supplied[key])
