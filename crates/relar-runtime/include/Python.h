/* Source compatibility for extensions rebuilt for the Relar runtime. */
#ifndef RELAR_PYTHON_H
#define RELAR_PYTHON_H

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#define RELAR_EXTERN_C extern "C"
#else
#define RELAR_EXTERN_C
#endif
#ifdef _WIN32
#define PyMODINIT_FUNC RELAR_EXTERN_C __declspec(dllexport) PyObject *
#else
#define PyMODINIT_FUNC RELAR_EXTERN_C __attribute__((visibility("default"))) PyObject *
#endif

typedef struct RelarPyObject PyObject;
typedef intptr_t Py_ssize_t;
typedef PyObject *(*PyCFunction)(PyObject *, PyObject *);
typedef struct PyMethodDef {
    const char *ml_name;
    PyCFunction ml_meth;
    int ml_flags;
    const char *ml_doc;
} PyMethodDef;

#define METH_VARARGS 0x0001
#define PyModuleDef_HEAD_INIT 0
typedef struct PyModuleDef {
    int m_base;
    const char *m_name;
    const char *m_doc;
    Py_ssize_t m_size;
    PyMethodDef *m_methods;
    void *m_slots;
    void *m_traverse;
    void *m_clear;
    void (*m_free)(void *);
} PyModuleDef;

PyObject *relar_py_module_create(PyModuleDef *);
PyObject *relar_py_none(void);
void relar_py_incref(PyObject *);
void relar_py_decref(PyObject *);
PyObject *relar_py_long_from_unsigned_long(unsigned long);
PyObject *relar_py_long_from_long(long);
PyObject *relar_py_list_get_item(PyObject *, Py_ssize_t);
int relar_py_list_set_item(PyObject *, Py_ssize_t, PyObject *);
int relar_py_list_append(PyObject *, PyObject *);
Py_ssize_t relar_py_list_size(PyObject *);
PyObject *relar_py_call_one_arg(PyObject *, PyObject *);
PyObject *relar_py_exception_class(int);
void relar_py_set_error(PyObject *, const char *);
int relar_py_parse_count(PyObject *, size_t);
PyObject *relar_py_tuple_item(PyObject *, size_t);
int relar_py_parse_unsigned_long(PyObject *, unsigned long *);
int relar_py_parse_bytes(PyObject *, const char **, Py_ssize_t *);

#define PyModule_Create relar_py_module_create
#define Py_None relar_py_none()
#define Py_INCREF relar_py_incref
#define Py_DECREF relar_py_decref
#define Py_XINCREF relar_py_incref
#define Py_XDECREF relar_py_decref
static inline void relar_py_clear(PyObject **slot) {
    PyObject *old = *slot;
    *slot = NULL;
    Py_XDECREF(old);
}
#define Py_CLEAR(slot) relar_py_clear(&(slot))
#define Py_RETURN_NONE return Py_None
#define PyLong_FromUnsignedLong relar_py_long_from_unsigned_long
#define PyLong_FromLong relar_py_long_from_long
#define PyList_GetItem relar_py_list_get_item
#define PyList_SetItem relar_py_list_set_item
#define PyList_Append relar_py_list_append
#define PyList_Size relar_py_list_size
#define PyObject_CallOneArg relar_py_call_one_arg
#define PyExc_RuntimeError relar_py_exception_class(10)
#define PyExc_TypeError relar_py_exception_class(13)
#define PyExc_ValueError relar_py_exception_class(8)
#define PyExc_SystemError relar_py_exception_class(16)
#define PyErr_SetString relar_py_set_error

/* Consume varargs using the C compiler's own va_list convention. */
static inline int PyArg_ParseTuple(PyObject *args, const char *format, ...) {
    size_t count = 0;
    const char *p;
    for (p = format; *p && *p != ':'; ++p) {
        if (*p == 'y' && p[1] == '#') ++p;
        else if (*p != 'O' && *p != 'k') {
            PyErr_SetString(PyExc_SystemError, "unsupported PyArg_ParseTuple format");
            return 0;
        }
        ++count;
    }
    if (!relar_py_parse_count(args, count)) return 0;
    va_list outputs;
    va_start(outputs, format);
    size_t i = 0;
    int ok = 1;
    for (p = format; i < count && ok; ++p, ++i) {
        PyObject *item = relar_py_tuple_item(args, i);
        if (*p == 'O') {
            *va_arg(outputs, PyObject **) = item;
        } else if (*p == 'k') {
            ok = relar_py_parse_unsigned_long(item, va_arg(outputs, unsigned long *));
        } else {
            ++p;
            const char **data = va_arg(outputs, const char **);
            Py_ssize_t *length = va_arg(outputs, Py_ssize_t *);
            ok = relar_py_parse_bytes(item, data, length);
        }
    }
    va_end(outputs);
    return ok;
}

#ifdef __cplusplus
}
#endif
#endif
