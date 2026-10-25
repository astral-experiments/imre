/* A fixture-only POSIX operation. It reaps the data child in its actual
 * parent, without using or updating the native Popen handle. */
#include <Python.h>
#include <errno.h>
#include <string.h>
#include <sys/wait.h>

static PyObject *raw_wait(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid, flags;
    if (!PyArg_ParseTuple(args, "kk:raw_wait", &pid, &flags)) return NULL;
    int status = 0;
    pid_t result;
    do {
        result = waitpid((pid_t)pid, &status, (int)flags);
    } while (result < 0 && errno == EINTR);
    if (result < 0) {
        PyErr_SetString(PyExc_RuntimeError, strerror(errno));
        return NULL;
    }
    /* A POSIX wait status occupies 16 bits. The host helper gets both fields. */
    return PyLong_FromUnsignedLong(((unsigned long)result << 16) | (status & 0xffff));
}

static PyMethodDef methods[] = {
    {"raw_wait", raw_wait, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "fixture_waitpid", NULL, -1, methods,
    NULL, NULL, NULL, NULL
};
PyMODINIT_FUNC PyInit_fixture_waitpid(void) { return PyModule_Create(&module); }
