#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <limits.h>
#include <sqlite3.h>
#include <string.h>

#if SQLITE_VERSION_NUMBER < 3035000
#error SQLite 3.35 or newer is required for RETURNING
#endif

/* Each bound UTF-8 value is prefixed by its decimal byte length and a colon.
 * Bind with an explicit length: a user-supplied NUL is ordinary TEXT data. */
static int bind_params(sqlite3_stmt *stmt, const char *data, Py_ssize_t size) {
    int count = sqlite3_bind_parameter_count(stmt);
    for (int i = 1; i <= count; i++) {
        const char *name = sqlite3_bind_parameter_name(stmt, i);
        if (name && name[0] != '?') {
            return SQLITE_RANGE;
        }
        int length = 0;
        if (!size || *data < '0' || *data > '9') {
            return SQLITE_RANGE;
        }
        while (size && *data >= '0' && *data <= '9') {
            int digit = *data - '0';
            if (length > (INT_MAX - digit) / 10) {
                return SQLITE_TOOBIG;
            }
            length = 10 * length + digit;
            ++data;
            --size;
        }
        if (size == 0 || *data != ':' || length > size - 1) {
            return SQLITE_RANGE;
        }
        int rc = sqlite3_bind_text(stmt, i, data + 1, length, SQLITE_STATIC);
        if (rc != SQLITE_OK) {
            return rc;
        }
        data += 1 + (Py_ssize_t)length;
        size -= 1 + (Py_ssize_t)length;
    }
    return size == 0 ? SQLITE_OK : SQLITE_RANGE;
}

/* A database may contain arbitrary bytes tagged as TEXT. Reject them before
 * committing a RETURNING statement, as CPython's sqlite3 text factory does. */
static int valid_utf8(const unsigned char *s, int size) {
    for (int i = 0; i < size;) {
        unsigned char lead = s[i++];
        if (lead < 0x80) {
            continue;
        }
        if (lead < 0xc2 || lead > 0xf4) {
            return 0;
        }
        int extra = lead < 0xe0 ? 1 : lead < 0xf0 ? 2 : 3;
        if (extra > size - i ||
            (lead == 0xe0 && s[i] < 0xa0) || (lead == 0xed && s[i] >= 0xa0) ||
            (lead == 0xf0 && s[i] < 0x90) || (lead == 0xf4 && s[i] >= 0x90)) {
            return 0;
        }
        while (extra--) {
            if ((s[i++] & 0xc0) != 0x80) {
                return 0;
            }
        }
    }
    return 1;
}

static PyObject *execute(PyObject *self, PyObject *args) {
    (void)self;
    const char *path, *sql, *params, *tail;
    Py_ssize_t path_size, sql_size, params_size;
    if (!PyArg_ParseTuple(args, "y#y#y#:execute", &path, &path_size, &sql, &sql_size, &params, &params_size)) {
        return NULL;
    }
    if (path_size == 0 || path_size > INT_MAX || sql_size > INT_MAX ||
        memchr(path, 0, (size_t)path_size) || (sql_size && memchr(sql, 0, (size_t)sql_size))) {
        PyErr_SetString(PyExc_ValueError, "invalid database path or SQL");
        return NULL;
    }
    char *filename = sqlite3_mprintf("%.*s", (int)path_size, path);
    if (!filename) {
        PyErr_SetString(PyExc_RuntimeError, "out of memory");
        return NULL;
    }
    sqlite3 *db = NULL;
    sqlite3_stmt *stmt = NULL;
    sqlite3_str *out = NULL;
    PyObject *result = NULL;
    int rc = sqlite3_open_v2(filename, &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, NULL);
    sqlite3_free(filename);
    if (rc != SQLITE_OK || sqlite3_busy_timeout(db, 5000) != SQLITE_OK ||
        sqlite3_exec(db, "PRAGMA foreign_keys=ON; BEGIN", NULL, NULL, NULL) != SQLITE_OK) {
        goto sql_error;
    }
    if (!sql_size) {
        sql = "";
    }
    rc = sqlite3_prepare_v2(db, sql, (int)sql_size, &stmt, &tail);
    if (rc != SQLITE_OK) {
        goto sql_error;
    }
    if (tail < sql + sql_size) {
        sqlite3_stmt *extra = NULL;
        rc = sqlite3_prepare_v2(db, tail, (int)(sql + sql_size - tail), &extra, NULL);
        if (rc != SQLITE_OK || extra) {
            sqlite3_finalize(extra);
            PyErr_SetString(PyExc_RuntimeError, "expected one SQLite statement");
            goto cleanup;
        }
    }
    rc = stmt ? bind_params(stmt, params, params_size) : (params_size ? SQLITE_RANGE : SQLITE_OK);
    if (rc != SQLITE_OK) {
        PyErr_SetString(PyExc_RuntimeError, sqlite3_errstr(rc));
        goto cleanup;
    }
    out = sqlite3_str_new(db);
    int width = stmt ? sqlite3_column_count(stmt) : 0;
    sqlite3_str_appendf(out, "%d:", width);
    while (stmt && (rc = sqlite3_step(stmt)) == SQLITE_ROW) {
        for (int i = 0; i < width; i++) {
            int type = sqlite3_column_type(stmt, i);
            if (type != SQLITE_INTEGER && type != SQLITE_TEXT) {
                PyErr_SetString(PyExc_ValueError, "SQLite results require INTEGER or TEXT columns");
                goto cleanup;
            }
            const unsigned char *text = sqlite3_column_text(stmt, i);
            if (!text) {
                goto sql_error;
            }
            int length = sqlite3_column_bytes(stmt, i);
            if (type == SQLITE_TEXT && !valid_utf8(text, length)) {
                PyErr_SetString(PyExc_RuntimeError, "invalid UTF-8 SQLite TEXT");
                goto cleanup;
            }
            sqlite3_str_appendf(out, "%d:", length);
            sqlite3_str_append(out, (const char *)text, length);
        }
    }
    if (stmt && rc != SQLITE_DONE) {
        goto sql_error;
    }
    if (sqlite3_str_errcode(out) != SQLITE_OK) {
        PyErr_SetString(PyExc_RuntimeError, sqlite3_errstr(sqlite3_str_errcode(out)));
        goto cleanup;
    }
    result = PyBytes_FromStringAndSize(sqlite3_str_value(out), sqlite3_str_length(out));
    if (!result) {
        goto cleanup;
    }
    if (sqlite3_exec(db, "COMMIT", NULL, NULL, NULL) != SQLITE_OK) {
        Py_DECREF(result);
        result = NULL;
        goto sql_error;
    }
    goto cleanup;

sql_error:
    PyErr_SetString(PyExc_RuntimeError, sqlite3_errmsg(db));
cleanup:
    sqlite3_free(sqlite3_str_finish(out));
    sqlite3_finalize(stmt);
    sqlite3_close(db); /* Rolls back any uncommitted transaction. */
    return result;
}

static PyMethodDef methods[] = {
    {"execute", execute, METH_VARARGS, "Execute one bound SQLite statement."},
    {NULL, NULL, 0, NULL},
};
static PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "_tracker_sqlite", NULL, -1, methods, NULL, NULL, NULL, NULL,
};
PyMODINIT_FUNC PyInit__tracker_sqlite(void) {
    return PyModule_Create(&module);
}
