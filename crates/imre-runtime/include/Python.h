/* Source compatibility for extensions rebuilt for the Imre runtime. */
#ifndef IMRE_PYTHON_H
#define IMRE_PYTHON_H

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#define IMRE_EXTERN_C extern "C"
#else
#define IMRE_EXTERN_C
#endif
#ifdef _WIN32
#define PyMODINIT_FUNC IMRE_EXTERN_C __declspec(dllexport) PyObject *
#else
#define PyMODINIT_FUNC IMRE_EXTERN_C __attribute__((visibility("default"))) PyObject *
#endif

typedef struct ImrePyObject PyObject;
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

PyObject *imre_py_module_create(PyModuleDef *);
PyObject *imre_py_none(void);
void imre_py_incref(PyObject *);
void imre_py_decref(PyObject *);
PyObject *imre_py_long_from_unsigned_long(unsigned long);
PyObject *imre_py_long_from_long(long);
PyObject *imre_py_bytes_from_string_and_size(const char *, Py_ssize_t);
PyObject *imre_py_list_get_item(PyObject *, Py_ssize_t);
int imre_py_list_set_item(PyObject *, Py_ssize_t, PyObject *);
int imre_py_list_append(PyObject *, PyObject *);
Py_ssize_t imre_py_list_size(PyObject *);
PyObject *imre_py_call_one_arg(PyObject *, PyObject *);
PyObject *imre_py_exception_class(int);
void imre_py_set_error(PyObject *, const char *);
int imre_py_parse_count(PyObject *, size_t);
PyObject *imre_py_tuple_item(PyObject *, size_t);
int imre_py_parse_unsigned_long(PyObject *, unsigned long *);
int imre_py_parse_bytes(PyObject *, const char **, Py_ssize_t *);

#define PyModule_Create imre_py_module_create
#define Py_None imre_py_none()
#define Py_INCREF imre_py_incref
#define Py_DECREF imre_py_decref
#define Py_XINCREF imre_py_incref
#define Py_XDECREF imre_py_decref
static inline void imre_py_clear(PyObject **slot) {
    PyObject *old = *slot;
    *slot = NULL;
    Py_XDECREF(old);
}
#define Py_CLEAR(slot) imre_py_clear(&(slot))
#define Py_RETURN_NONE return Py_None
#define PyLong_FromUnsignedLong imre_py_long_from_unsigned_long
#define PyLong_FromLong imre_py_long_from_long
#define PyBytes_FromStringAndSize imre_py_bytes_from_string_and_size
#define PyList_GetItem imre_py_list_get_item
#define PyList_SetItem imre_py_list_set_item
#define PyList_Append imre_py_list_append
#define PyList_Size imre_py_list_size
#define PyObject_CallOneArg imre_py_call_one_arg
#define PyExc_RuntimeError imre_py_exception_class(10)
#define PyExc_TypeError imre_py_exception_class(13)
#define PyExc_ValueError imre_py_exception_class(8)
#define PyExc_SystemError imre_py_exception_class(16)
#define PyErr_SetString imre_py_set_error

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
    if (!imre_py_parse_count(args, count)) return 0;
    va_list outputs;
    va_start(outputs, format);
    size_t i = 0;
    int ok = 1;
    for (p = format; i < count && ok; ++p, ++i) {
        PyObject *item = imre_py_tuple_item(args, i);
        if (*p == 'O') {
            *va_arg(outputs, PyObject **) = item;
        } else if (*p == 'k') {
            ok = imre_py_parse_unsigned_long(item, va_arg(outputs, unsigned long *));
        } else {
            ++p;
            const char **data = va_arg(outputs, const char **);
            Py_ssize_t *length = va_arg(outputs, Py_ssize_t *);
            ok = imre_py_parse_bytes(item, data, length);
        }
    }
    va_end(outputs);
    return ok;
}

#ifdef __cplusplus
}
#endif
#endif
