/* A fixture-only POSIX operation. It reaps the data child in its actual
 * parent, without using or updating the native Popen handle. */
#define _GNU_SOURCE
#include <Python.h>
#include <errno.h>
#include <string.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <sys/wait.h>

/* Only the original host mock's dynamic scope arms this interception. Every
 * other wait goes to the kernel. Popen itself owns decoding and caching. */
static pid_t mock_pid;
pid_t waitpid(pid_t pid, int *status, int options) {
    if (mock_pid > 0 && pid == mock_pid) {
        mock_pid = 0;
        char response[100];
        int result, value;
        if (printf("mock-waitpid %d %d\n", (int)pid, options) < 0 ||
            fflush(stdout) != 0 || fgets(response, sizeof(response), stdin) == NULL ||
            sscanf(response, "%d %d", &result, &value) != 2 ||
            result != pid || status == NULL) {
            errno = EINVAL;
            return -1;
        }
        *status = value;
        return (pid_t)result;
    }
    return (pid_t)syscall(SYS_wait4, pid, status, options, NULL);
}

static PyObject *mock_next(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid;
    if (!PyArg_ParseTuple(args, "k:mock_next", &pid)) return NULL;
    mock_pid = (pid_t)pid;
    return PyLong_FromLong(0);
}
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
    {"mock_next", mock_next, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "fixture_waitpid", NULL, -1, methods,
    NULL, NULL, NULL, NULL
};
PyMODINIT_FUNC PyInit_fixture_waitpid(void) { return PyModule_Create(&module); }
