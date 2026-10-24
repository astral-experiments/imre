#include <Python.h>
#include <build_config.h>

#if SETUP_VALUE != EXPECTED_VALUE
#error "missing the compile options from pyproject.toml"
#endif

PyObject *retainer_init(void);

PyMODINIT_FUNC PyInit_retainer(void) {
    return retainer_init();
}
