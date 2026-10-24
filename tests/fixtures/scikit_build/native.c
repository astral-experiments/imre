#define PY_SSIZE_T_CLEAN
#include <Python.h>

static PyObject *popcount(PyObject *self, PyObject *args) {
    const char *bytes;
    Py_ssize_t length;
    if (!PyArg_ParseTuple(args, "y#:popcount", &bytes, &length)) {
        return NULL;
    }
    unsigned long bits = 0;
    for (Py_ssize_t i = 0; i < length; i++) {
        unsigned char byte = (unsigned char)bytes[i];
        while (byte) {
            byte &= byte - 1;
            bits++;
        }
    }
    return PyLong_FromUnsignedLong(bits);
}

static PyMethodDef methods[] = {
    {"popcount", popcount, METH_VARARGS, "Count the set bits in a byte string."},
    {NULL, NULL, 0, NULL},
};

static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "_native", NULL, -1, methods,
};

PyMODINIT_FUNC PyInit__native(void) {
    return PyModule_Create(&module);
}
