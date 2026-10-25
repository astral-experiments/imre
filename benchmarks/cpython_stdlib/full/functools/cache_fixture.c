/* Reenter the native test dispatcher when a host callback invokes its wrapper.
   Built against Imre's extension-header, without libpython. */
#include <Python.h>

static PyObject *dispatcher;

static PyObject *install(PyObject *self, PyObject *args) {
    PyObject *value;
    if (!PyArg_ParseTuple(args, "O:install", &value)) return NULL;
    Py_INCREF(value);
    Py_CLEAR(dispatcher);
    dispatcher = value;
    Py_RETURN_NONE;
}

static PyObject *nested(PyObject *self, PyObject *args) {
    PyObject *command;
    if (!PyArg_ParseTuple(args, "O:nested", &command)) return NULL;
    return PyObject_CallOneArg(dispatcher, command);
}

static void release(void *module) { Py_CLEAR(dispatcher); }

static PyMethodDef methods[] = {
    {"install", install, METH_VARARGS, NULL},
    {"nested", nested, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};

static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "cache_fixture", NULL, -1, methods, NULL, NULL, NULL, release
};

PyMODINIT_FUNC PyInit_cache_fixture(void) { return PyModule_Create(&module); }
