/* The unchanged os.kill / os.waitpid fixtures must operate in the actual
 * kernel parent. Neither syscall uses or modifies Imre's Popen handle. */
#define _POSIX_C_SOURCE 200809L
#include <Python.h>
#include <errno.h>
#include <signal.h>
#include <sys/wait.h>

static PyObject *send_signal(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid, sig;
    if (!PyArg_ParseTuple(args, "kk:send_signal", &pid, &sig)) return NULL;
    int rc = kill((pid_t)pid, (int)sig);
    return PyLong_FromLong(rc < 0 ? -errno : 0);
}

static PyObject *wait_status(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid, flags;
    if (!PyArg_ParseTuple(args, "kk:wait_status", &pid, &flags)) return NULL;
    int status = 0;
    pid_t rc;
    do {
        rc = waitpid((pid_t)pid, &status, (int)flags);
    } while (rc < 0 && errno == EINTR);
    if (rc < 0) return PyLong_FromLong(-errno);
    return PyLong_FromUnsignedLong(((unsigned long)rc << 16) | (status & 0xffff));
}

static PyMethodDef methods[] = {
    {"send_signal", send_signal, METH_VARARGS, NULL},
    {"wait_status", wait_status, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "fixture_active_os", NULL, -1, methods,
    NULL, NULL, NULL, NULL
};
PyMODINIT_FUNC PyInit_fixture_active_os(void) { return PyModule_Create(&module); }
