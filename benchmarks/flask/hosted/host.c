#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <frameobject.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

/* Experimental host for Imre's boxed LLVM ABI. The caller holds the GIL. */
typedef uintptr_t Id;
typedef struct {
    PyObject *owned;
    PyObject *globals;
    PyObject *factory;
    const char *file;
    unsigned line;
} Frame;
typedef Id (*Body)(Frame *, Id);
extern int imre_register_program(int argc, char **argv);

static PyObject *object(Id id) {
    if (id == 0) return Py_None;
    if (id == 1) return Py_False;
    if (id == 2) return Py_True;
    return (PyObject *)id;
}

static Id own(Frame *frame, PyObject *obj) {
    if (!obj) return 0;
    if (PyList_Append(frame->owned, obj) < 0) {
        Py_DECREF(obj);
        return 0;
    }
    Id id = (Id)obj;
    Py_DECREF(obj);
    return id;
}

/* Generated main only registers code. Its opaque context is the module registry. */
void *imre_dyn_new(int argc, char **argv, const char *config) { return (void *)argv; }
void imre_dyn_register(PyObject *registry, const char *name, const char *file, Body run) {
    if (PyErr_Occurred()) return;
    PyObject *code = PyCapsule_New((void *)run, "imre.llvm.body", NULL);
    if (code) {
        PyDict_SetItemString(registry, name, code);
        Py_DECREF(code);
    }
}
int imre_dyn_finish(void *unused) { return PyErr_Occurred() ? 1 : 0; }
bool imre_dyn_failed(Frame *frame) { return PyErr_Occurred() != NULL; }
void imre_dyn_statement(Frame *frame, const Id *roots, Id count, const char *file, unsigned line) {
    frame->file = file;
    frame->line = line;
}

Id imre_dyn_lit(Frame *frame, int kind, const char *text, Id length) {
    if (kind == 0) return own(frame, PyLong_FromString(text, NULL, 10));
    if (kind == 1) return own(frame, PyUnicode_DecodeUTF8(text, (Py_ssize_t)length, "strict"));
    if (kind == 2) return own(frame, PyBytes_FromStringAndSize(text, (Py_ssize_t)length));
    PyErr_SetString(PyExc_SystemError, "unsupported literal");
    return 0;
}

Id imre_dyn_op(Frame *frame, const char *op, Id a, Id b, Id c) {
    PyObject *x = object(a), *y = object(b), *z = object(c);
    if (PyErr_Occurred()) return 0;
    if (strcmp(op, "lookup") == 0) {
        PyObject *val = NULL;
        if (PyDict_GetItemRef(x, y, &val) < 0) return 0;
        if (!val && x != frame->globals &&
            PyDict_GetItemRef(frame->globals, y, &val) < 0) return 0;
        if (!val && PyDict_GetItemRef(PyEval_GetBuiltins(), y, &val) < 0) return 0;
        if (!val) PyErr_Format(PyExc_NameError, "name %R is not defined", y);
        return own(frame, val);
    }
    if (strcmp(op, "assign") == 0) return PyDict_SetItem(x, y, z) < 0 ? 0 : c;
    if (strcmp(op, "import") == 0) return own(frame, PyImport_Import(y));
    if (strcmp(op, "attr") == 0 || strcmp(op, "from") == 0)
        return own(frame, PyObject_GetAttr(x, y));
    if (strcmp(op, "get") == 0) return own(frame, PyObject_GetItem(x, y));
    if (strcmp(op, "put") == 0) return PyObject_SetItem(x, y, z) < 0 ? 0 : c;
    if (strcmp(op, "keyword") == 0) {
        int present = PyDict_Contains(x, y);
        if (present < 0) return 0;
        if (present) { PyErr_Format(PyExc_TypeError, "duplicate keyword %R", y); return 0; }
        return PyDict_SetItem(x, y, z) < 0 ? 0 : c;
    }
    if (strcmp(op, "list") == 0) return own(frame, PyList_New(0));
    if (strcmp(op, "dict") == 0) return own(frame, PyDict_New());
    if (strcmp(op, "append") == 0) return PyList_Append(x, y) < 0 ? 0 : a;
    if (strcmp(op, "tuple") == 0) return own(frame, PySequence_Tuple(x));
    if (strcmp(op, "+") == 0) return own(frame, PyNumber_Add(x, y));
    if (strcmp(op, "call") == 0) {
        PyObject *args = PySequence_Tuple(y);
        if (!args) return 0;
        PyObject *value = PyObject_Call(x, args, z);
        Py_DECREF(args);
        return own(frame, value);
    }
    PyErr_Format(PyExc_NotImplementedError, "hosted LLVM operation %s", op);
    return 0;
}

Id imre_dyn_def(Frame *frame, Body run, Id scope, Id name, Id signature, Id defaults) {
    PyObject *code = PyCapsule_New((void *)run, "imre.llvm.body", NULL);
    if (!code) return 0;
    PyObject *func = PyObject_CallFunctionObjArgs(frame->factory, code, object(scope),
        object(name), object(signature), object(defaults), NULL);
    Py_DECREF(code);
    return own(frame, func);
}

/* Attribute a Python exception to the last source statement executed in LLVM. */
static void add_source_frame(Frame *frame, const char *name) {
    if (!frame->file) return;
    PyObject *original = PyErr_GetRaisedException();
    PyCodeObject *code = PyCode_NewEmpty(frame->file, name, (int)frame->line);
    PyFrameObject *trace = code ? PyFrame_New(PyThreadState_Get(), code, frame->globals, NULL) : NULL;
    Py_XDECREF(code);
    PyErr_Clear();
    PyErr_SetRaisedException(original);
    if (trace) {
        PyTraceBack_Here(trace);
        Py_DECREF(trace);
    }
}

static PyObject *execute(PyObject *capsule, const char *name, PyObject *scope,
                         PyObject *globals, PyObject *factory) {
    Body run = (Body)PyCapsule_GetPointer(capsule, "imre.llvm.body");
    if (!run) return NULL;
    if (!PyDict_Check(scope) || !PyDict_Check(globals)) {
        PyErr_SetString(PyExc_TypeError, "compiled scopes must be dictionaries");
        return NULL;
    }
    Frame frame = {.owned = PyList_New(0), .globals = globals, .factory = factory};
    if (!frame.owned) return NULL;
    Id id = run(&frame, (Id)scope);
    if (PyErr_Occurred()) add_source_frame(&frame, name);
    PyObject *value = PyErr_Occurred() ? NULL : Py_NewRef(object(id));
    Py_DECREF(frame.owned);
    return value;
}

static PyObject *load_module(PyObject *self, PyObject *args) {
    const char *name;
    PyObject *scope, *factory;
    if (!PyArg_ParseTuple(args, "sOO", &name, &scope, &factory)) return NULL;
    PyObject *registry = PyObject_GetAttrString(self, "_module_code");
    if (!registry) return NULL;
    PyObject *code = NULL;
    int found = PyDict_GetItemStringRef(registry, name, &code);
    Py_DECREF(registry);
    if (found <= 0) {
        if (!found) PyErr_Format(PyExc_ImportError, "no LLVM module %s", name);
        return NULL;
    }
    PyObject *result = execute(code, "<module>", scope, scope, factory);
    Py_DECREF(code);
    return result;
}

static PyObject *invoke(PyObject *self, PyObject *args) {
    PyObject *capsule, *scope, *globals, *factory;
    const char *name;
    if (!PyArg_ParseTuple(args, "OsOOO", &capsule, &name, &scope, &globals, &factory)) return NULL;
    return execute(capsule, name, scope, globals, factory);
}

static PyMethodDef methods[] = {
    {"load_module", load_module, METH_VARARGS, "Execute the LLVM module initializer."},
    {"invoke", invoke, METH_VARARGS, "Invoke an Imre-generated LLVM function."},
    {NULL, NULL, 0, NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "_imre_views", NULL, -1, methods
};
PyMODINIT_FUNC PyInit__imre_views(void) {
    PyObject *registry = PyDict_New();
    if (!registry) return NULL;
    PyObject *mod = imre_register_program(0, (char **)registry) ? NULL : PyModule_Create(&module);
    if (mod && PyModule_AddObjectRef(mod, "_module_code", registry) < 0) Py_CLEAR(mod);
    Py_DECREF(registry);
    return mod;
}
