/* Transfer only fixture state: the original no-op SIGUSR1 disposition and
 * an actual alias for the native pipe. No read, launch, poll or wait here. */
#define _POSIX_C_SOURCE 200809L
#include <Python.h>
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static volatile sig_atomic_t deliveries;
static volatile sig_atomic_t channel;
static struct sigaction previous;
static int armed;
static void count(int signum) {
    int saved = errno;
    unsigned char byte = (unsigned char)signum;
    if (write((int)channel, &byte, 1) != 1) _exit(121);
    ++deliveries;
    errno = saved;
}

static PyObject *arm(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long fd;
    if (!PyArg_ParseTuple(args, "k:arm", &fd)) return NULL;
    if (fd < 3 || fd > INT_MAX) {
        PyErr_SetString(PyExc_RuntimeError, "invalid fixture notification channel"); return NULL;
    }
    channel = (sig_atomic_t)fd;
    struct sigaction action = {0};
    action.sa_handler = count;
    if (armed || sigemptyset(&action.sa_mask) || sigaction(SIGUSR1, &action, &previous)) {
        PyErr_SetString(PyExc_RuntimeError, "cannot install native SIGUSR1 fixture");
        return NULL;
    }
    armed = 1; deliveries = 0;
    Py_RETURN_NONE;
}
static PyObject *disarm(PyObject *self, PyObject *args) {
    (void)self;
    if (!PyArg_ParseTuple(args, ":disarm")) return NULL;
    if (!armed || sigaction(SIGUSR1, &previous, NULL)) {
        PyErr_SetString(PyExc_RuntimeError, "cannot restore native SIGUSR1 fixture");
        return NULL;
    }
    armed = 0;
    return PyLong_FromLong(deliveries);
}
static PyObject *share_fd(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long channel, source;
    if (!PyArg_ParseTuple(args, "kk:share_fd", &channel, &source)) return NULL;
    int fd = (int)source;
    union { struct cmsghdr align; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    char byte = 'F';
    struct iovec data = {&byte, 1};
    struct msghdr msg = {0};
    msg.msg_iov = &data; msg.msg_iovlen = 1;
    msg.msg_control = control.bytes; msg.msg_controllen = sizeof(control.bytes);
    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_level = SOL_SOCKET; cmsg->cmsg_type = SCM_RIGHTS; cmsg->cmsg_len = CMSG_LEN(sizeof(fd));
    memcpy(CMSG_DATA(cmsg), &fd, sizeof(fd));
    if (sendmsg((int)channel, &msg, 0) != 1) {
        PyErr_SetString(PyExc_RuntimeError, strerror(errno)); return NULL;
    }
    Py_RETURN_NONE;
}
static PyMethodDef methods[] = {
    {"arm",arm,METH_VARARGS,NULL},{"disarm",disarm,METH_VARARGS,NULL},
    {"share_fd",share_fd,METH_VARARGS,NULL},{NULL,NULL,0,NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "fixture_pipes", NULL, -1, methods, NULL,NULL,NULL,NULL
};
PyMODINIT_FUNC PyInit_fixture_pipes(void) { return PyModule_Create(&module); }

