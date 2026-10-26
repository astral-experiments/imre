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
static PyObject *observer;
static unsigned long observed_calls;

pid_t waitpid(pid_t pid, int *status, int options) {
    if (mock_pid > 0 && pid == mock_pid) {
        if (observer != NULL) {
            /* Record this native wait in Imre before consulting the original host patch. */
            PyObject *flags = PyLong_FromLong(options);
            PyObject *answer = flags ? PyObject_CallOneArg(observer, flags) : NULL;
            Py_XDECREF(flags);
            if (answer == NULL) {
                errno = EINVAL;
                return -1;
            }
            Py_DECREF(answer);
            ++observed_calls;
        } else {
            mock_pid = 0;
        }
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

static PyObject *enter_mock(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid;
    PyObject *callback;
    if (!PyArg_ParseTuple(args, "kO:enter_mock", &pid, &callback)) return NULL;
    if (mock_pid != 0 || pid == 0) {
        PyErr_SetString(PyExc_RuntimeError, "waitpid target in use or PID is zero");
        return NULL;
    }
    Py_INCREF(callback);
    observer = callback;
    mock_pid = (pid_t)pid;
    observed_calls = 0;
    Py_RETURN_NONE;
}

static PyObject *leave_mock(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid;
    if (!PyArg_ParseTuple(args, "k:leave_mock", &pid)) return NULL;
    if (mock_pid != (pid_t)pid || observer == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "no matching installed waitpid target");
        return NULL;
    }
    mock_pid = 0;
    Py_CLEAR(observer);
    return PyLong_FromUnsignedLong(observed_calls);
}

static PyObject *wait_errno(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long pid, flags;
    if (!PyArg_ParseTuple(args, "kk:wait_errno", &pid, &flags)) return NULL;
    int status;
    pid_t result;
    do {
        result = waitpid((pid_t)pid, &status, (int)flags);
    } while (result < 0 && errno == EINTR);
    return PyLong_FromLong(result < 0 ? errno : 0);
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
    {"enter_mock", enter_mock, METH_VARARGS, NULL},
    {"leave_mock", leave_mock, METH_VARARGS, NULL},
    {"wait_errno", wait_errno, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "fixture_waitpid", NULL, -1, methods,
    NULL, NULL, NULL, NULL
};
PyMODINIT_FUNC PyInit_fixture_waitpid(void) { return PyModule_Create(&module); }
