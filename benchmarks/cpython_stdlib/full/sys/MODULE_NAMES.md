# Standard library module names

`sys.stdlib_module_names` describes the Python language target. It includes
modules unavailable on the current platform. Membership does not imply that
the Imre compiler can import the module.

The [runtime table](../../../../crates/imre-runtime/native/stdlib-module-names.txt)
contains the 297 top-level names from CPython's
[Python/stdlib_module_names.h](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Python/stdlib_module_names.h),
blob `56e349a544c07971287625a740eb9f5dd566bdc6`, under the
[vendored CPython license](../../upstream/LICENSE). The sys runner checks that
blob and every table entry when passed a CPython source tree. With a sparse
Git checkout, it reads the same blob from the pinned tree if the header is
absent on disk.

The [native control](module_names.py) compares all names with stock CPython.
It checks identity across import forms, membership (including `winreg` and
`email.message`), and the concrete frozen set's type, repr, and equality.
The control adds no upstream test ID.
