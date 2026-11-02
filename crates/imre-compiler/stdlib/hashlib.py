# From CPython 3.14.5 Lib/hashlib.py (PSF-2.0). See stdlib/docstrings/LICENSE.
__all__ = (
    'md5', 'sha1', 'sha224', 'sha256', 'sha384', 'sha512', 'blake2b', 'blake2s',
    'sha3_224', 'sha3_256', 'sha3_384', 'sha3_512', 'shake_128', 'shake_256',
    'new', 'algorithms_guaranteed', 'algorithms_available', 'file_digest',
)

__builtin_constructor_cache = {}


def __get_builtin_constructor(name):
    cache = __builtin_constructor_cache
    constructor = cache.get(name)
    if constructor is not None:
        return constructor
    try:
        if name in {'SHA1', 'sha1'}:
            import _sha1
            cache['SHA1'] = cache['sha1'] = _sha1.sha1
        elif name in {'MD5', 'md5'}:
            import _md5
            cache['MD5'] = cache['md5'] = _md5.md5
        elif name in {'SHA256', 'sha256', 'SHA224', 'sha224'}:
            import _sha2
            cache['SHA224'] = cache['sha224'] = _sha2.sha224
            cache['SHA256'] = cache['sha256'] = _sha2.sha256
        elif name in {'SHA512', 'sha512', 'SHA384', 'sha384'}:
            import _sha2
            cache['SHA384'] = cache['sha384'] = _sha2.sha384
            cache['SHA512'] = cache['sha512'] = _sha2.sha512
        elif name in {'blake2b', 'blake2s'}:
            import _blake2
            cache['blake2b'] = _blake2.blake2b
            cache['blake2s'] = _blake2.blake2s
        elif name in {'sha3_224', 'sha3_256', 'sha3_384', 'sha3_512'}:
            import _sha3
            cache['sha3_224'] = _sha3.sha3_224
            cache['sha3_256'] = _sha3.sha3_256
            cache['sha3_384'] = _sha3.sha3_384
            cache['sha3_512'] = _sha3.sha3_512
        elif name in {'shake_128', 'shake_256'}:
            import _sha3
            cache['shake_128'] = _sha3.shake_128
            cache['shake_256'] = _sha3.shake_256
    except ImportError:
        pass  # no extension module, this hash is unsupported.

    constructor = cache.get(name)
    if constructor is not None:
        return constructor

    raise ValueError('unsupported hash type ' + name)


__block_openssl_constructor = {'blake2b', 'blake2s'}
__sha3_provider_names = {
    'sha3_224', 'sha3_256', 'sha3_384', 'sha3_512',
    'shake_128', 'shake_256',
}


def __hash_new(name, *args, **kwargs):
    """new(name, data=b'') - Return a new hashing object using the named algorithm;
    optionally initialized with data (which must be a bytes-like object).
    """
    if name in __block_openssl_constructor or name in __sha3_provider_names:
        return __get_builtin_constructor(name)(*args, **kwargs)
    if isinstance(name, str) and str.lower(name) in {'md5', 'ssl3-md5', '1.2.840.113549.2.5'}:
        if _hashlib is not None:
            try:
                return _hashlib.new(name, *args, **kwargs)
            except ValueError:
                pass
        elif name not in {'MD5', 'md5'}:
            # Imre also supports OpenSSL's MD5 aliases with its portable backend.
            name = 'md5'
        return __get_builtin_constructor(name)(*args, **kwargs)
    try:
        return _native_new(name, *args, **kwargs)
    except ValueError:
        return __get_builtin_constructor(name)(*args, **kwargs)


new = __hash_new
del __hash_new


# The named constructors keep the provider selected at import time. new()
# looks up the live builtin constructor each time, as in CPython's fallback.
for __func_name in __block_openssl_constructor | __sha3_provider_names:
    try:
        globals()[__func_name] = __get_builtin_constructor(__func_name)
    except ValueError:
        pass
del __func_name


try:
    import _hashlib
except ImportError:
    _hashlib = None

try:
    _hashlib.openssl_md5(usedforsecurity=False)
except (AttributeError, ValueError):
    try:
        md5 = __get_builtin_constructor('md5')
    except ValueError:
        pass
else:
    md5 = _hashlib.openssl_md5

try:
    from _hashlib import pbkdf2_hmac
    __all__ += ('pbkdf2_hmac',)
except ImportError:
    pass

try:
    from _hashlib import scrypt
except ImportError:
    pass
