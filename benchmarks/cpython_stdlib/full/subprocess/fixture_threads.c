/* Schedule calls to the same native child-control ABI as the compiled Popen.
 * This fixture has no process launch, waitpid, kill, or status cache of its own. */
#define _GNU_SOURCE
#include <Python.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

extern void *__real_imre_popen_control(void *popen);
extern uintptr_t imre_child_status(const void *control, bool poll);
extern void imre_child_signal(const void *control, uintptr_t signal);
extern uintptr_t imre_child_wait(const void *control, unsigned kind, uintptr_t timeout, void *owner);
extern unsigned imre_error_pending(void);
static void *last_control;
static pthread_t thread;
static bool started;

/* Observe the owner thread borrowing its real control; keep the Popen rooted
 * in the compiled caller until join. Only the separate control reaches the worker. */
void *__wrap_imre_popen_control(void *popen) {
    last_control = __real_imre_popen_control(popen);
    return last_control;
}

struct worker { const void *control; int fd; };

static void *serve(void *arg) {
    struct worker w = *(struct worker *)arg;
    free(arg);
    FILE *in = fdopen(dup(w.fd), "r"), *out = fdopen(w.fd, "w");
    if (!in || !out) abort();
    fprintf(out, "%ld\n", syscall(SYS_gettid));
    fflush(out);
    char line[64];
    while (fgets(line, sizeof(line), in)) {
        uintptr_t word = 0;
        if (!strcmp(line, "status\n")) word = imre_child_status(w.control, false);
        else if (!strcmp(line, "poll\n")) word = imre_child_status(w.control, true);
        else if (!strcmp(line, "wait\n")) word = imre_child_wait(w.control, 0, 0, NULL);
        else if (!strcmp(line, "kill\n")) {
            imre_child_signal(w.control, ((uintptr_t)9 << 1) | 1);
            word = 1;
        } else if (!strcmp(line, "quit\n")) break;
        else abort();
        if (imre_error_pending()) abort();
        if (word == 0) fputs("None\n", out);
        else fprintf(out, "%ld\n", (long)((intptr_t)word >> 1));
        fflush(out);
    }
    fclose(in); fclose(out);
    return NULL;
}

static PyObject *start_worker(PyObject *self, PyObject *args) {
    (void)self;
    unsigned long fd;
    if (!PyArg_ParseTuple(args, "k:start", &fd)) return NULL;
    if (!last_control || started) abort();
    struct worker *w = malloc(sizeof(*w));
    if (!w) abort();
    *w = (struct worker){last_control, (int)fd};
    if (pthread_create(&thread, NULL, serve, w)) abort();
    started = true;
    Py_RETURN_NONE;
}

static PyObject *join_worker(PyObject *self, PyObject *args) {
    (void)self;
    if (!PyArg_ParseTuple(args, ":join")) return NULL;
    if (!started || pthread_join(thread, NULL)) abort();
    started = false;
    Py_RETURN_NONE;
}
static PyMethodDef methods[] = {
    {"start", start_worker, METH_VARARGS, NULL},
    {"join", join_worker, METH_VARARGS, NULL},
    {NULL, NULL, 0, NULL}
};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "fixture_threads", NULL, -1, methods, NULL, NULL, NULL, NULL
};
PyMODINIT_FUNC PyInit_fixture_threads(void) { return PyModule_Create(&module); }
