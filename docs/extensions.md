# Rebuilding extension modules

Relar compiles suitable C extensions against its own `Python.h`. Applications import the resulting modules with ordinary Python syntax. Each installed module supplies a typed stub and a static archive. Relar links the archive into the application when it builds.

## Build a wheel

The retainer fixture is a complete extension that keeps a list and calls a Python function. From the root of a Relar checkout, build it with the same compiler version that will build the application. The packing script uses the Python packages `wheel` and `packaging`.

```sh
mkdir -p build/include build/stage
relar extension-header > build/include/Python.h
cc -O2 -fPIC -Ibuild/include -c tests/fixtures/extensions/retainer.c -o build/retainer.o
ar rcs build/stage/retainer.relar.a build/retainer.o
cp tests/fixtures/extensions/retainer.pyi build/stage/
python scripts/pack_extension_wheel.py build/stage --name retainer --version 1.0 --output dist
```

For a package module such as `checksums._native`, put `_native.pyi` and `_native.relar.a` in `checksums/` alongside a supported `__init__.py`. The archive exports `PyInit__native`. An optional `_native.relar.libs` file lists one system link library per line, such as `stdc++` on Linux. Native modules in one executable must have distinct initialization symbols.

## Install and use it

The wheel tag is `rl1-relar1-<platform>`. It identifies Relar’s first extension interface and the platform for which the archive was built. Use the host platform by default when installing. With the wheel above, these commands install it in an isolated directory and run an application that imports it.

```sh
python -m pip install --no-index --no-deps --no-compile --only-binary=:all: \
  --implementation rl --python-version 1 --abi relar1 --target build/site dist/retainer-*.whl
relar run tests/fixtures/extensions/retained.py --extension-path build/site
```

Supply `--extension-path` again for additional installed directories. CPython wheels have a different ABI. Builds using an extension path reuse cached module objects and relink the executable, so replacing an installed archive or its link libraries takes effect on the next build. A finished Relar executable can run after the installation directory is removed.

## The C boundary

Module initialization uses `PyModule_Create` and a `METH_VARARGS` method table. In its `.pyi` file, each exported function declares positional-only arguments and a return type. Supported native values are `None`, `int`, `bytes`, `list[int]`, and an integer callback of type `Callable[[int], int]`. The header printed by `relar extension-header` is the authoritative list of available operations.

- `PyArg_ParseTuple` handles `O`, `k`, and `y#`, including a `:function` suffix. A read-only bytes view remains valid while the extension holds the object.
- `Py_INCREF` and `Py_DECREF` let an extension retain arguments beyond the call. A retained list shares the original storage. Foreign writes preserve `list[int]` and a rejected write raises `TypeError`.
- `PyObject_CallOneArg` invokes a retained integer callback, including a bound method. The callback can call the extension again. Its exceptions propagate to the Python caller.
- A returned object transfers an owned reference. Relar validates it against the declared return type. A module can release retained references in its `m_free` hook.

Extensions run on the calling thread with opaque handles. This ABI has no public object layout, thread attachment, custom Python type creation, or cyclic collector. Arbitrary CPython extensions can depend on those facilities.
