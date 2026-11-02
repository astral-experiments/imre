/* Fatal reporting must not allocate or enter the Python heap or stdio. */
#define _GNU_SOURCE
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#if defined(__linux__) || defined(__APPLE__)
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <ucontext.h>
#include <unistd.h>
#if defined(__linux__)
#include <fcntl.h>
#endif

enum { MAX_THREADS = 100, MAX_FRAMES = 100, MAX_CHARS = 500 };
_Static_assert(ATOMIC_INT_LOCK_FREE == 2 && ATOMIC_LLONG_LOCK_FREE == 2,
               "fatal reporting requires lock-free signal-time atomics");
typedef struct {
    char utf8[4 * MAX_CHARS];
    uint16_t len;
    int truncated;
} Text;
typedef struct {
    int present;
    uint32_t line;
    Text file, name;
} Frame;
typedef struct {
    atomic_uint busy;
    Frame *frames;
    pthread_t thread;
    pid_t pid;
    char name[32];
    size_t depth, capacity;
    void *alt_stack;
} Slot;

/* A reporter tries each gate once and copies before writing: a blocked pipe
 * must never block a worker's frame bookkeeping. */
static Slot slots[MAX_THREADS];
/* A handler may inherit a small application-provided alternate stack. Keep
 * the bounded frame copies off that stack, including for a nested signal. */
static struct { atomic_uint used; Frame frame; } scratch[MAX_THREADS];
#if defined(__linux__)
/* The fatal handler must be able to read even an untouched thread's TLS
 * without calling a resolver that might allocate. */
static _Thread_local _Atomic(Slot *) local __attribute__((tls_model("initial-exec")));
#else
static _Thread_local _Atomic(Slot *) local;
#endif
static pthread_key_t key;
static pthread_once_t key_once = PTHREAD_ONCE_INIT;
static int key_ready;
static atomic_uint generation;
static unsigned next_generation;
static atomic_ullong settings;
static pthread_mutex_t config_lock = PTHREAD_MUTEX_INITIALIZER;
static const char hex[] = "0123456789abcdef";

static void unlock(Slot *s) { atomic_store_explicit(&s->busy, 0, memory_order_release); }
static int try_lock(Slot *s) {
    return atomic_exchange_explicit(&s->busy, 1, memory_order_acquire) == 0;
}
/* Only normal execution waits. The interrupted thread may itself own a gate. */
static void lock(Slot *s) { while (!try_lock(s)) {} }

static void drop_slot(void *value) {
    Slot *s = value;
    lock(s);
    Frame *frames = s->frames;
    void *alt = s->alt_stack;
    s->alt_stack = NULL;
    s->frames = NULL;
    s->depth = 0;
    s->capacity = 0;
    atomic_store_explicit(&local, NULL, memory_order_relaxed);
    unlock(s);
    free(frames);
    if (alt) {
        stack_t saved;
        if (sigaltstack(NULL, &saved) != 0) return;
        if (saved.ss_sp == alt && !(saved.ss_flags & SS_DISABLE)) {
            stack_t off = { .ss_flags = SS_DISABLE };
            if (sigaltstack(&off, NULL) != 0) return;
        }
        free(alt);
    }
}
static void make_key(void) { key_ready = pthread_key_create(&key, drop_slot) == 0; }

static Slot *thread_slot(void) {
    Slot *current = atomic_load_explicit(&local, memory_order_relaxed);
    if (current) return current;
    pthread_once(&key_once, make_key);
    if (!key_ready) return NULL;
    Frame *frames = calloc(1, sizeof(Frame));
    if (!frames) return NULL;
    for (size_t i = 0; i < MAX_THREADS; i++) {
        Slot *s = &slots[i];
        lock(s);
        if (!s->frames && pthread_setspecific(key, s) == 0) {
            s->frames = frames;
            s->capacity = 1;
            s->thread = pthread_self();
            s->pid = getpid();
            s->name[0] = 0;
            pthread_getname_np(s->thread, s->name, sizeof(s->name));
            atomic_store_explicit(&local, s, memory_order_relaxed);
            unlock(s);
            return s;
        }
        unlock(s);
    }
    free(frames);
    return NULL;
}

/* Both inputs are valid UTF-8. Limit by code points, before escaping. */
static void copy_text(Text *out, const char *src, size_t len) {
    size_t pos = 0;
    for (size_t chars = 0; pos < len && chars < MAX_CHARS; chars++) {
        unsigned byte = (unsigned char)src[pos];
        pos += byte < 128 ? 1 : byte < 224 ? 2 : byte < 240 ? 3 : 4;
    }
    memcpy(out->utf8, src, pos);
    out->len = (uint16_t)pos;
    out->truncated = pos < len;
}
uint32_t imre_fault_generation(void) {
    return atomic_load_explicit(&generation, memory_order_acquire);
}
void imre_fault_frame(size_t index, const char *file, size_t file_len,
                      const char *name, size_t name_len, uint32_t line) {
    Slot *s = thread_slot();
    if (!s) return;
    lock(s);
    s->pid = getpid();
    /* Keep callers past the report limit. They become visible on unwind. */
    if (index >= s->depth) s->depth = index + 1;
    if (index >= s->capacity) {
        size_t n = s->capacity * 2;
        if (n <= index) n = index + 1;
        if (n == 0 || n > SIZE_MAX / sizeof(Frame)) { unlock(s); return; }
        Frame *frames = realloc(s->frames, n * sizeof(Frame));
        if (!frames) { unlock(s); return; }
        memset(frames + s->capacity, 0, (n - s->capacity) * sizeof(Frame));
        s->frames = frames;
        s->capacity = n;
    }
    Frame *f = &s->frames[index];
    copy_text(&f->file, file, file_len);
    copy_text(&f->name, name, name_len);
    f->line = line;
    f->present = 1;
    unlock(s);
}
void imre_fault_line(size_t index, uint32_t line) {
    Slot *s = atomic_load_explicit(&local, memory_order_relaxed);
    if (s) {
        lock(s);
        if (index < s->depth && index < s->capacity) s->frames[index].line = line;
        unlock(s);
    }
}
void imre_fault_truncate(size_t depth) {
    Slot *s = atomic_load_explicit(&local, memory_order_relaxed);
    if (s) {
        lock(s);
        if (depth < s->depth) s->depth = depth;
        unlock(s);
    }
}

static void emit(int fd, const char *p, size_t n) {
    while (n) {
        ssize_t k = write(fd, p, n);
        if (k < 0 && errno == EINTR) continue;
        if (k <= 0) break;
        p += k;
        n -= (size_t)k;
    }
}
#define LIT(fd, s) emit(fd, s, sizeof(s)-1)
static void number(int fd, uintptr_t value, unsigned radix, size_t width) {
    char buf[3 * sizeof(uintptr_t)]; size_t n = sizeof(buf);
    do { buf[--n] = hex[value % radix]; value /= radix; } while (value || sizeof(buf)-n < width);
    emit(fd, buf+n, sizeof(buf)-n);
}
static void emit_text(int fd, const Text *text) {
    char out[96];
    size_t pos = 0;
    for (size_t i = 0; i < text->len;) {
        if (pos + 10 > sizeof(out)) { emit(fd, out, pos); pos = 0; }
        unsigned c = (unsigned char)text->utf8[i++];
        if (c >= 32 && c < 127) { out[pos++] = (char)c; continue; }
        if (c >= 128) {
            unsigned n = c < 224 ? 1 : c < 240 ? 2 : 3;
            c &= 0x7f >> (n + 1);
            while (n--) c = (c << 6) | ((unsigned char)text->utf8[i++] & 63);
        }
        unsigned digits = c <= 255 ? 2 : c <= 65535 ? 4 : 8;
        out[pos++] = '\\'; out[pos++] = digits == 2 ? 'x' : digits == 4 ? 'u' : 'U';
        for (unsigned j = digits; j; j--) out[pos++] = hex[(c >> ((j-1)*4)) & 15];
    }
    emit(fd, out, pos);
    if (text->truncated) LIT(fd, "...");
}
static void thread_header(int fd, pthread_t tid, const char *name, int current) {
    if (current) LIT(fd, "Current thread 0x"); else LIT(fd, "Thread 0x");
    number(fd, (uintptr_t)tid, 16, 2*sizeof(uintptr_t));
    if (name && *name) { LIT(fd, " ["); emit(fd, name, strlen(name)); LIT(fd, "]"); }
    LIT(fd, " (most recent call first):\n");
}

/* A single bounded stack copy guards the lifetime of a worker or a frame
 * that is returning on another thread while this report is being written. */
static int dump_slot(Slot *s, int fd, int all, int preceding, Frame *f) {
    if (!try_lock(s)) return 0;
#if defined(__linux__)
    /* The caller's slot survives fork, even before it publishes its new PID. */
    int current = s == atomic_load_explicit(&local, memory_order_relaxed);
#else
    /* Darwin may allocate on first TLS access. Keep the fatal path TLS-free. */
    int current = s->frames && s->pid == getpid() && pthread_equal(s->thread, pthread_self());
#endif
    if (!s->frames || (!current && (!all || !s->depth || s->pid != getpid()))) {
        unlock(s);
        return 0;
    }
    size_t depth = s->depth;
    pthread_t thread = s->thread;
    char name[sizeof(s->name)];
    memcpy(name, s->name, sizeof(name));
    unlock(s);
    if (preceding) LIT(fd, "\n");
    if (all) thread_header(fd, thread, name, current);
    else LIT(fd, "Stack (most recent call first):\n");
    if (!depth) LIT(fd, "  <no Python frame>\n");
    size_t count = depth > MAX_FRAMES ? MAX_FRAMES : depth;
    for (size_t i = 0; i < count; i++) {
        size_t index = depth-1-i;
        if (!try_lock(s)) { LIT(fd, "  ...\n"); break; }
        int valid = s->frames && pthread_equal(s->thread, thread) && index < s->depth &&
                    index < s->capacity && s->frames[index].present;
        if (valid) *f = s->frames[index];
        unlock(s);
        if (!valid) { LIT(fd, "  ...\n"); break; }
        LIT(fd, "  File \""); emit_text(fd, &f->file);
        LIT(fd, "\", line "); number(fd, f->line, 10, 0);
        LIT(fd, " in "); emit_text(fd, &f->name); LIT(fd, "\n");
    }
    return current ? 2 : 1;
}
void imre_fault_dump(int fd, int all_threads) {
    size_t buf = 0;
    while (buf < MAX_THREADS &&
           atomic_exchange_explicit(&scratch[buf].used, 1, memory_order_acquire)) buf++;
    if (buf == MAX_THREADS) { LIT(fd, "  <traceback unavailable>\n"); return; }
    int preceding = 0, current = 0;
    for (size_t i = MAX_THREADS; i; i--) {
        int which = dump_slot(&slots[i - 1], fd, all_threads, preceding, &scratch[buf].frame);
        if (which) preceding = 1;
        if (which == 2) current = 1;
    }
    if (!current) {
        if (preceding) LIT(fd, "\n");
        if (all_threads) thread_header(fd, pthread_self(), NULL, 1);
        else LIT(fd, "Stack (most recent call first):\n");
        LIT(fd, "  <no Python frame>\n");
    }
    atomic_store_explicit(&scratch[buf].used, 0, memory_order_release);
}

static void native_stack(int fd, void *context) {
    LIT(fd, "\nCurrent thread's C stack trace (most recent call first):\n");
    uintptr_t pc = 0, fp = 0, sp = 0;
    ucontext_t *uc = context;
#if defined(__linux__) && defined(__x86_64__) && defined(REG_RIP)
    pc = uc->uc_mcontext.gregs[REG_RIP];
    fp = uc->uc_mcontext.gregs[REG_RBP];
    sp = uc->uc_mcontext.gregs[REG_RSP];
#elif defined(__linux__) && defined(__aarch64__)
    pc = uc->uc_mcontext.pc;
    fp = uc->uc_mcontext.regs[29];
    sp = uc->uc_mcontext.sp;
#elif defined(__APPLE__) && defined(__x86_64__)
    pc = uc->uc_mcontext->__ss.__rip;
#elif defined(__APPLE__) && defined(__aarch64__) && defined(__darwin_arm_thread_state64_get_pc)
    pc = __darwin_arm_thread_state64_get_pc(uc->uc_mcontext->__ss);
#else
    (void)uc;
#endif
    if (!pc) { LIT(fd, "  <native stack unavailable>\n"); return; }
#if defined(__linux__)
    int memory = open("/proc/self/mem", O_RDONLY | O_CLOEXEC);
#endif
    for (unsigned i = 0; i < MAX_FRAMES; i++) {
        LIT(fd, "  <unknown binary> [0x"); number(fd, pc, 16, 0); LIT(fd, "]\n");
#if defined(__linux__)
        /* pread copies through the kernel. A corrupt frame pointer yields an
         * error instead of a second fault. Neither stack probing nor address
         * formatting needs an allocator, unwinder or dynamic loader lock. */
        if (memory < 0 || fp < sp || fp-sp > 64*1024*1024 || fp % sizeof(uintptr_t)) break;
        uintptr_t record[2];
        if (pread(memory, record, sizeof(record), (off_t)fp) != sizeof(record)) break;
        if (!record[1]) break;
        pc = record[1];
        sp = fp + sizeof(record);
        fp = record[0];
#else
        (void)fp; (void)sp;
        break;
#endif
    }
#if defined(__linux__)
    if (memory >= 0) close(memory);
#endif
}

static const struct { int signum; const char *name; } signals[] = {
    {SIGBUS, "Bus error"}, {SIGILL, "Illegal instruction"}, {SIGFPE, "Floating-point exception"},
    {SIGABRT, "Aborted"}, {SIGSEGV, "Segmentation fault"}
};
enum { NSIGNALS = sizeof(signals)/sizeof(signals[0]) };
typedef struct { struct sigaction saved[NSIGNALS]; } Dispositions;
/* A handler pins this immutable copy only until its disposition is restored.
 * Configuration changes never wait for diagnostic output or a user handler. */
static _Atomic(Dispositions *) originals;
static atomic_uint readers;
_Static_assert(ATOMIC_POINTER_LOCK_FREE == 2, "signal-time disposition loads must not lock");

static void fatal(int sig, siginfo_t *info, void *context) {
    (void)info;
    int saved_errno = errno;
    size_t i = 0;
    while (i < NSIGNALS && signals[i].signum != sig) i++;
    if (i == NSIGNALS) return;
    atomic_fetch_add(&readers, 1);
    Dispositions *d = atomic_load(&originals);
    sigaction(sig, &d->saved[i], NULL);
    atomic_fetch_sub(&readers, 1);
    unsigned long long opts = atomic_load(&settings);
    int fd = (int)(uint32_t)opts;
    LIT(fd, "Fatal Python error: ");
    emit(fd, signals[i].name, strlen(signals[i].name));
    LIT(fd, "\n\n");
    imre_fault_dump(fd, (opts >> 32) & 1);
    if ((opts >> 33) & 1) native_stack(fd, context);
    errno = saved_errno;
    /* SA_NODEFER lets the original disposition receive this signal immediately
     * on this same thread, even if writing a broken pipe also queued SIGPIPE. */
    raise(sig);
}
static void alternate_stack(void) {
    stack_t old;
    if (sigaltstack(NULL, &old) != 0 || !(old.ss_flags & SS_DISABLE)) return;
    Slot *s = thread_slot();
    if (!s || s->alt_stack) return;
    size_t size = SIGSTKSZ;
    if (size < 65536) size = 65536;
    void *buf = malloc(size);
    if (!buf) return;
    stack_t stack = { .ss_sp = buf, .ss_size = size };
    if (sigaltstack(&stack, NULL) == 0) s->alt_stack = buf;
    else free(buf);
}

int imre_fault_enable(int fd, int all_threads, int c_stack) {
    if (fd < 0) { errno = EINVAL; return -1; }
    pthread_mutex_lock(&config_lock);
    unsigned long long opts = (uint32_t)fd | ((unsigned long long)!!all_threads << 32)
                                          | ((unsigned long long)!!c_stack << 33);
    if (imre_fault_generation()) {
        atomic_store(&settings, opts);
        pthread_mutex_unlock(&config_lock);
        return 0;
    }
    Dispositions *d = malloc(sizeof(*d));
    if (!d) goto fail;
    for (size_t i = 0; i < NSIGNALS; i++) {
        if (sigaction(signals[i].signum, NULL, &d->saved[i]) != 0) {
            free(d); goto fail;
        }
    }
    alternate_stack();
    Dispositions *old = atomic_exchange(&originals, d);
    while (atomic_load(&readers)) {}
    free(old);
    atomic_store(&settings, opts);
    struct sigaction action = { .sa_sigaction = fatal, .sa_flags = SA_SIGINFO | SA_ONSTACK | SA_NODEFER };
    sigemptyset(&action.sa_mask);
    sigaddset(&action.sa_mask, SIGPIPE);
    for (size_t i = 0; i < NSIGNALS; i++) {
        if (sigaction(signals[i].signum, &action, NULL) != 0) {
            int error = errno;
            while (i) { i--; sigaction(signals[i].signum, &d->saved[i], NULL); }
            errno = error;
            goto fail;
        }
    }
    if (++next_generation == 0) next_generation++;
    atomic_store_explicit(&generation, next_generation, memory_order_release);
    pthread_mutex_unlock(&config_lock);
    return 0;
fail:
    pthread_mutex_unlock(&config_lock);
    return -1;
}
int imre_fault_disable(void) {
    pthread_mutex_lock(&config_lock);
    int enabled = atomic_exchange(&generation, 0) != 0;
    if (enabled) {
        Dispositions *d = atomic_load(&originals);
        for (size_t i = 0; i < NSIGNALS; i++) sigaction(signals[i].signum, &d->saved[i], NULL);
    }
    pthread_mutex_unlock(&config_lock);
    return enabled;
}
#else
/* Native signal management is implemented for Imre's Linux and macOS hosts. */
int imre_fault_enable(int fd, int all, int stack) { (void)fd;(void)all;(void)stack;errno=ENOSYS;return -1; }
int imre_fault_disable(void) { return 0; }
uint32_t imre_fault_generation(void) { return 0; }
void imre_fault_frame(size_t i,const char*f,size_t fl,const char*n,size_t nl,uint32_t l)
    { (void)i;(void)f;(void)fl;(void)n;(void)nl;(void)l; }
void imre_fault_line(size_t i,uint32_t l) { (void)i;(void)l; }
void imre_fault_truncate(size_t d) { (void)d; }
void imre_fault_dump(int fd,int all) { (void)fd;(void)all; }
#endif
