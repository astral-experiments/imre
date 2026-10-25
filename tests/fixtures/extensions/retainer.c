#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <stdint.h>

static PyObject *retained_list;
static PyObject *retained_callback;
static PyObject *retained_args;

static void clear_retained(void *module) {
    (void)module;
    Py_CLEAR(retained_list);
    Py_CLEAR(retained_callback);
    Py_CLEAR(retained_args);
}

static PyObject *remember(PyObject *self, PyObject *args) {
    PyObject *items, *callback;
    if (!PyArg_ParseTuple(args, "OO:remember", &items, &callback))
        return NULL;
    Py_INCREF(items);
    Py_INCREF(callback);
    clear_retained(NULL);
    retained_list = items;
    retained_callback = callback;
    Py_RETURN_NONE;
}

static PyObject *same(PyObject *self, PyObject *args) {
    PyObject *candidate;
    if (!PyArg_ParseTuple(args, "O:same", &candidate))
        return NULL;
    return PyLong_FromUnsignedLong(candidate == retained_list);
}

static PyObject *first(PyObject *self, PyObject *args) {
    if (!PyArg_ParseTuple(args, ":first"))
        return NULL;
    if (retained_list == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "no retained list");
        return NULL;
    }
    PyObject *value = PyList_GetItem(retained_list, 0);
    if (value == NULL)
        return NULL;
    Py_INCREF(value);
    return value;
}

static PyObject *replace_and_call(PyObject *self, PyObject *args) {
    PyObject *value;
    if (!PyArg_ParseTuple(args, "O:replace_and_call", &value))
        return NULL;
    if (retained_list == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "no retained list");
        return NULL;
    }
    Py_INCREF(value);  /* PyList_SetItem steals this reference, including on failure. */
    if (PyList_SetItem(retained_list, 0, value) < 0)
        return NULL;
    /* The callback may re-enter the extension and release or replace itself. */
    PyObject *callback = retained_callback;
    Py_INCREF(callback);
    PyObject *result = PyObject_CallOneArg(callback, value);
    Py_DECREF(callback);
    return result;
}

static PyObject *store_bytes(PyObject *self, PyObject *args) {
    PyObject *value;
    if (!PyArg_ParseTuple(args, "O:store_bytes", &value))
        return NULL;
    if (retained_list == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "no retained list");
        return NULL;
    }
    Py_INCREF(value);
    if (PyList_SetItem(retained_list, 0, value) < 0)
        return NULL;
    Py_RETURN_NONE;
}

static PyObject *release(PyObject *self, PyObject *args) {
    if (!PyArg_ParseTuple(args, ":release"))
        return NULL;
    clear_retained(NULL);
    Py_RETURN_NONE;
}

/* Exercise a copied C buffer, a slice without a terminator and size errors. */
static PyObject *buffer_bytes(PyObject *self, PyObject *args) {
    const char *data;
    Py_ssize_t length;
    unsigned long mode;
    if (!PyArg_ParseTuple(args, "y#k:buffer_bytes", &data, &length, &mode)) return NULL;
    if (mode == 1) return PyBytes_FromStringAndSize(NULL, length);
    if (mode == 2) return PyBytes_FromStringAndSize(NULL, -1);
    if (mode == 3) return PyBytes_FromStringAndSize(NULL, INTPTR_MAX);
    if (mode == 4) {
        char local[] = {'x', 0, (char)0xff};
        PyObject *owned = PyBytes_FromStringAndSize(local + 1, 2);
        ((volatile char *)local)[1] = 'y';
        return owned;
    }
    return PyBytes_FromStringAndSize(data, length);
}

/* A false declaration in the installed stub must fail at the native boundary. */
static PyObject *wrong_return(PyObject *self, PyObject *args) {
    PyObject *value;
    if (!PyArg_ParseTuple(args, "O:wrong_return", &value))
        return NULL;
    Py_INCREF(value);
    return value;
}

static PyObject *pin_args(PyObject *self, PyObject *args) {
    Py_INCREF(args);
    Py_CLEAR(retained_args);
    retained_args = args;
    Py_RETURN_NONE;
}

static PyObject *pinned_value(PyObject *self, PyObject *args) {
    PyObject *value;
    if (retained_args == NULL || !PyArg_ParseTuple(retained_args, "O", &value))
        return NULL;
    Py_INCREF(value);
    return value;
}

static PyMethodDef methods[] = {
    {"remember", remember, METH_VARARGS, NULL},
    {"same", same, METH_VARARGS, NULL},
    {"first", first, METH_VARARGS, NULL},
    {"replace_and_call", replace_and_call, METH_VARARGS, NULL},
    {"store_bytes", store_bytes, METH_VARARGS, NULL},
    {"buffer_bytes", buffer_bytes, METH_VARARGS, NULL},
    {"wrong_return", wrong_return, METH_VARARGS, NULL},
    {"pin_args", pin_args, METH_VARARGS, NULL},
    {"pinned_value", pinned_value, METH_VARARGS, NULL},
    {"release", release, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};

static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "retainer", NULL, -1, methods, NULL, NULL, NULL, clear_retained
};

PyMODINIT_FUNC PyInit_retainer(void) {
    return PyModule_Create(&module);
}
