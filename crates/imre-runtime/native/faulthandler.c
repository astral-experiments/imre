/* Fatal reporting must not allocate or enter the Python heap or stdio. */
#define _GNU_SOURCE
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#if defined(__linux__) || defined(__APPLE__)
#include <pthread.h>
#include <sched.h>
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
    uint64_t revision;
    uint32_t line;
    Text file, name;
} Frame;
/* A fork keeps one pthread; its in-flight operations must retain their gates. */
_Static_assert(__atomic_always_lock_free(sizeof(uintptr_t), 0),
               "signal-time gate ownership must be lock-free");
typedef _Atomic(uintptr_t) Gate;
typedef struct {
    Gate busy;
    Frame *frames;
    pthread_t thread;
    /* Both numbers belong to the slot and survive the exit of its owner. */
    uint64_t revision, incarnation;
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
static struct { Gate used; Frame frame; } scratch[MAX_THREADS];
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
static Gate config_owner;
static void recover_config(uintptr_t self);
static const char hex[] = "0123456789abcdef";

/* Called under the slot gate. Zero marks an unavailable record after exhaustion. */
static uint64_t next_revision(Slot *s) {
    return s->revision == UINT64_MAX ? 0 : ++s->revision;
}

static void unlock(Slot *s) { atomic_store_explicit(&s->busy, 0, memory_order_release); }
static int try_gate(Gate *gate, uintptr_t thread) {
    uintptr_t expected = 0;
    return atomic_compare_exchange_strong_explicit(gate, &expected, thread,
                                                   memory_order_acquire, memory_order_relaxed);
}
/* Only normal execution waits. The interrupted thread may itself own a gate. */
static void lock(Slot *s, pthread_t thread) { while (!try_gate(&s->busy, (uintptr_t)thread)) {} }

static void drop_slot(void *value) {
    Slot *s = value;
    lock(s, s->thread);
    Frame *frames = s->frames;
    void *alt = s->alt_stack;
    s->thread = (pthread_t)0;
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
/* The child cannot wait for vanished publishers. A busy foreign slot can be
 * between realloc and publication of the new pointer, so abandon its storage.
 * Quiescent orphans are reclaimed on the next normal claim, outside this hook.
 * No prepare handler waits for a thread whose progress might depend on fork. */
static void after_fork(void) {
    pthread_t thread = pthread_self();
    uintptr_t self = (uintptr_t)thread;
    pid_t pid = getpid();
    sigset_t mask, previous;
    sigfillset(&mask);
    sigprocmask(SIG_BLOCK, &mask, &previous);
    recover_config(self);
    for (size_t i = 0; i < MAX_THREADS; i++) {
        Slot *s = &slots[i];
        uintptr_t busy = atomic_load_explicit(&s->busy, memory_order_relaxed);
        if (s->thread == thread) {
            s->pid = pid;
        } else {
            if (busy && busy != self) {
                s->frames = NULL;
                s->alt_stack = NULL;
            }
            /* A later child pthread may reuse this old identifier before its own fork. */
            s->thread = (pthread_t)0;
            s->depth = 0;
            s->pid = 0;
        }
        if (busy != self) unlock(s);
        if (atomic_load_explicit(&scratch[i].used, memory_order_relaxed) != self) {
            atomic_store_explicit(&scratch[i].used, 0, memory_order_relaxed);
        }
    }
    sigprocmask(SIG_SETMASK, &previous, NULL);
}
static void make_key(void) {
    if (pthread_key_create(&key, drop_slot) != 0) return;
    if (pthread_atfork(NULL, NULL, after_fork) == 0) key_ready = 1;
    else pthread_key_delete(key);
}

static Slot *thread_slot(void) {
    Slot *current = atomic_load_explicit(&local, memory_order_relaxed);
    if (current) return current;
    pthread_once(&key_once, make_key);
    if (!key_ready) return NULL;
    Frame *frames = calloc(1, sizeof(Frame));
    if (!frames) return NULL;
    pthread_t thread = pthread_self();
    for (size_t i = 0; i < MAX_THREADS; i++) {
        Slot *s = &slots[i];
        lock(s, thread);
        if ((!s->frames || !s->pid) && s->revision != UINT64_MAX &&
            pthread_setspecific(key, s) == 0) {
            Frame *orphan = s->frames;
            void *alt = s->alt_stack;
            s->alt_stack = NULL;
            s->depth = 0;
            s->incarnation = next_revision(s);
            s->frames = frames;
            s->capacity = 1;
            s->thread = thread;
            s->pid = getpid();
            s->name[0] = 0;
            pthread_getname_np(s->thread, s->name, sizeof(s->name));
            atomic_store_explicit(&local, s, memory_order_relaxed);
            unlock(s);
            if (orphan) free(orphan);
            if (alt) free(alt);
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
    lock(s, s->thread);
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
    f->revision = next_revision(s);
    unlock(s);
}
void imre_fault_line(size_t index, uint32_t line) {
    Slot *s = atomic_load_explicit(&local, memory_order_relaxed);
    if (s) {
        lock(s, s->thread);
        if (index < s->depth && index < s->capacity) {
            Frame *f = &s->frames[index];
            if (f->revision && f->line != line) {
                f->line = line;
                f->revision = next_revision(s);
            }
        }
        unlock(s);
    }
}
void imre_fault_truncate(size_t depth) {
    Slot *s = atomic_load_explicit(&local, memory_order_relaxed);
    if (s) {
        lock(s, s->thread);
        if (depth < s->depth) {
            /* A later sparse publication must not expose calls that have returned. */
            size_t end = s->depth < s->capacity ? s->depth : s->capacity;
            for (size_t i = depth; i < end; i++) s->frames[i].revision = 0;
            s->depth = depth;
        }
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
static int dump_slot(Slot *s, int fd, int all, int preceding, Frame *f, uintptr_t self) {
    if (!try_gate(&s->busy, self)) return 0;
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
    uint64_t incarnation = s->incarnation;
    pthread_t thread = s->thread;
    char name[sizeof(s->name)];
    memcpy(name, s->name, sizeof(name));
    unlock(s);
    if (preceding) LIT(fd, "\n");
    if (all) thread_header(fd, thread, name, current);
    else LIT(fd, "Stack (most recent call first):\n");
    /* The header write can block. Select the stack now, only for that owner. */
    if (!try_gate(&s->busy, self)) goto unavailable;
    int same_owner = s->frames && s->incarnation == incarnation;
    size_t depth = s->depth;
    uint64_t revision = s->revision;
    unlock(s);
    if (!same_owner) goto unavailable;
    if (!depth) LIT(fd, "  <no Python frame>\n");
    size_t count = depth > MAX_FRAMES ? MAX_FRAMES : depth;
    for (size_t i = 0; i < count; i++) {
        size_t index = depth-1-i;
        if (!try_gate(&s->busy, self)) goto unavailable;
        /* Only a record still present from the selected stack may be emitted. */
        int valid = index < s->depth && index < s->capacity &&
                    s->frames[index].revision && s->frames[index].revision <= revision;
        if (valid) *f = s->frames[index];
        unlock(s);
        if (!valid) goto unavailable;
        LIT(fd, "  File \""); emit_text(fd, &f->file);
        LIT(fd, "\", line "); number(fd, f->line, 10, 0);
        LIT(fd, " in "); emit_text(fd, &f->name); LIT(fd, "\n");
    }
    return current ? 2 : 1;

unavailable:
    LIT(fd, "  ...\n");
    return current ? 2 : 1;
}
void imre_fault_dump(int fd, int all_threads) {
    uintptr_t self = (uintptr_t)pthread_self();
    size_t buf = 0;
    while (buf < MAX_THREADS && !try_gate(&scratch[buf].used, self)) buf++;
    if (buf == MAX_THREADS) { LIT(fd, "  <traceback unavailable>\n"); return; }
    int preceding = 0, current = 0;
    for (size_t i = MAX_THREADS; i; i--) {
        int which = dump_slot(&slots[i - 1], fd, all_threads, preceding, &scratch[buf].frame, self);
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

/* A fork from a nested application handler must run outside the pin. Restore
 * the interrupted mask before any dump, redelivery or predecessor callback. */
static void pin(atomic_uint *readers, sigset_t *previous) {
    sigset_t mask;
    sigfillset(&mask);
    sigprocmask(SIG_BLOCK, &mask, previous);
    atomic_fetch_add(readers, 1);
}
static void unpin(atomic_uint *readers, const sigset_t *previous) {
    atomic_fetch_sub(readers, 1);
    sigprocmask(SIG_SETMASK, previous, NULL);
}

static void fatal(int sig, siginfo_t *info, void *context) {
    (void)info;
    int saved_errno = errno;
    size_t i = 0;
    while (i < NSIGNALS && signals[i].signum != sig) i++;
    if (i == NSIGNALS) return;
    sigset_t previous;
    pin(&readers, &previous);
    Dispositions *d = atomic_load(&originals);
    sigaction(sig, &d->saved[i], NULL);
    unpin(&readers, &previous);
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

/* A delivery borrows the destination just as enable() does. Readers only pin
 * configuration while copying options or changing the kernel disposition.
 * In particular, unregister never waits on output or an arbitrary predecessor. */
typedef struct {
    struct sigaction previous;
    uint64_t serial;
    int fd, all, chain;
} UserAction;
typedef struct {
    _Atomic(UserAction *) active;
    atomic_uint readers;
    atomic_ullong restored;
    Gate restorer;
} UserSignal;
static UserSignal user_signals[NSIG];
static uint64_t user_serial;
static unsigned user_count;
enum { ALL_FATAL = (1u << NSIGNALS) - 1 };
static atomic_uint fatal_mask;

/* A normal writer exchanges one immutable allocation at a time. If it
 * vanishes at fork, the published pointer determines which allocation is
 * still owned. The other can be freed on the child's next normal operation. */
static struct {
    atomic_int signal;  /* Positive for a user registration, -1 for the fatal set. */
    void *old, *next;
} transfer;
static void *discard_after_fork;

static int lock_config(void) {
    pthread_once(&key_once, make_key);  /* Also needed with an external alt stack. */
    if (!key_ready) { errno = EAGAIN; return -1; }
    uintptr_t self = (uintptr_t)pthread_self();
    while (!try_gate(&config_owner, self)) sched_yield();
    void *discard = discard_after_fork;
    discard_after_fork = NULL;
    free(discard);
    return 0;
}

static void unlock_config(void) {
    atomic_store_explicit(&config_owner, 0, memory_order_release);
}

static void begin_transfer(int sig, void *old, void *next) {
    transfer.old = old;
    transfer.next = next;
    atomic_store(&transfer.signal, sig);
}

static void update_publication(void) {
    if (atomic_load(&fatal_mask) || user_count) {
        if (!imre_fault_generation()) {
            if (++next_generation == 0) next_generation++;
            atomic_store_explicit(&generation, next_generation, memory_order_release);
        }
    } else {
        atomic_store_explicit(&generation, 0, memory_order_release);
    }
}

static void recover_config(uintptr_t self) {
    /* Nested application signals can fork only after the short pin ends. */
    atomic_store(&readers, 0);
    for (int i = 1; i < NSIG; i++) {
        atomic_store(&user_signals[i].readers, 0);
        if (atomic_load(&user_signals[i].restorer) != self) {
            atomic_store(&user_signals[i].restored, 0);
        }
    }
    if (atomic_load(&config_owner) == self) return;
    int sig = atomic_load(&transfer.signal);
    if (sig) {
        void *published;
        if (sig > 0) {
            /* Keep the kernel's forked disposition. During an initial
             * sigaction its predecessor may still reside inside libc. */
            published = atomic_load(&user_signals[sig].active);
            user_count = 0;
            for (int i = 1; i < NSIG; i++) {
                if (atomic_load(&user_signals[i].active)) user_count++;
            }
        } else {
            published = atomic_load(&originals);
            unsigned mask = 0;
            for (size_t i = 0; i < NSIGNALS; i++) {
                struct sigaction action;
                if (sigaction(signals[i].signum, NULL, &action) == 0 &&
                    (action.sa_flags & SA_SIGINFO) && action.sa_sigaction == fatal) mask |= 1u << i;
            }
            atomic_store(&fatal_mask, mask);
        }
        if (transfer.old != transfer.next) {
            discard_after_fork = published == transfer.next ? transfer.old : transfer.next;
        }
        atomic_store(&transfer.signal, 0);
        update_publication();
    }
    unlock_config();
}

int imre_fault_enabled(void) { return atomic_load(&fatal_mask) != 0; }

static void user_signal(int sig);
static struct sigaction user_action(int chain) {
    struct sigaction action = {.sa_handler = user_signal,
        .sa_flags = SA_ONSTACK | (chain ? SA_NODEFER : SA_RESTART)};
    sigemptyset(&action.sa_mask);
    return action;
}

static UserAction *read_user(UserSignal *slot, sigset_t *previous) {
    pin(&slot->readers, previous);
    return atomic_load(&slot->active);
}

static void user_signal(int sig) {
    int saved_errno = errno;
    UserSignal *slot = &user_signals[sig];
    sigset_t previous;
    UserAction *active = read_user(slot, &previous);
    if (!active) {
        unpin(&slot->readers, &previous);
        return;
    }
    uint64_t serial = active->serial;
    int fd = active->fd, all = active->all;
    /* A fork during first installation can inherit our inactive handler.
     * Keep that exact action for unregister without redelivering to ourselves. */
    int chain = active->chain && active->previous.sa_handler != user_signal;
    unpin(&slot->readers, &previous);
    imre_fault_dump(fd, all);
    if (chain) {
        active = read_user(slot, &previous);
        int deliver = active && active->serial == serial;
        if (deliver) {
            atomic_store(&slot->restorer, (uintptr_t)pthread_self());
            atomic_store(&slot->restored, serial);
            if (sigaction(sig, &active->previous, NULL) < 0) {
                atomic_store(&slot->restored, 0);
                deliver = 0;
            }
        }
        unpin(&slot->readers, &previous);
        if (deliver) {
            errno = saved_errno;
            raise(sig);
            saved_errno = errno;
            active = read_user(slot, &previous);
            if (active && active->serial == serial) {
                struct sigaction action = user_action(chain);
                sigaction(sig, &action, NULL);
                atomic_store(&slot->restored, 0);
            }
            unpin(&slot->readers, &previous);
        }
    }
    errno = saved_errno;
}

int imre_fault_register(int sig, int fd, int all, int chain) {
    if (sig <= 0 || sig >= NSIG || fd < 0) { errno = EINVAL; return -1; }
    if (lock_config() < 0) return -1;
    UserSignal *slot = &user_signals[sig];
    UserAction *old = atomic_load(&slot->active);
    UserAction *next = NULL;
    if (user_serial == UINT64_MAX) { errno = EOVERFLOW; goto fail; }
    next = malloc(sizeof(*next));
    if (!next) goto fail;
    next->serial = ++user_serial;
    next->fd = fd;
    next->all = all;
    next->chain = chain;
    struct sigaction action = user_action(chain);
    begin_transfer(sig, old, next);
    if (old) next->previous = old->previous;
    else {
        alternate_stack();
        if (sigaction(sig, &action, &next->previous) < 0) goto fail;
    }
    atomic_store(&slot->active, next);
    while (atomic_load(&slot->readers)) {}
    /* Replacing options normally leaves the existing disposition alone. If a
     * previous delivery restored its predecessor, that delivery cannot rearm
     * this new registration on return. Install the new action here instead. */
    if (old && atomic_load(&slot->restored) == old->serial) {
        if (sigaction(sig, &action, NULL) < 0) {
            int error = errno;
            atomic_store(&slot->active, old);
            while (atomic_load(&slot->readers)) {}
            errno = error;
            goto fail;
        }
    }
    if (!old) user_count++;
    update_publication();
    atomic_store(&transfer.signal, 0);
    free(old);
    unlock_config();
    return 0;
fail: {
        int error = errno;
        atomic_store(&transfer.signal, 0);
        free(next);
        unlock_config();
        errno = error;
        return -1;
    }
}

static int unregister_user(int sig) {
    UserSignal *slot = &user_signals[sig];
    UserAction *old = atomic_load(&slot->active);
    if (!old) return 0;
    begin_transfer(sig, old, NULL);
    atomic_store(&slot->active, NULL);
    while (atomic_load(&slot->readers)) {}
    if (sigaction(sig, &old->previous, NULL) < 0) {
        atomic_store(&slot->active, old);
        atomic_store(&transfer.signal, 0);
        return -1;
    }
    atomic_store(&slot->restored, 0);
    user_count--;
    update_publication();
    atomic_store(&transfer.signal, 0);
    free(old);
    return 1;
}

int imre_fault_unregister(int sig) {
    if (sig <= 0 || sig >= NSIG) { errno = EINVAL; return -1; }
    if (lock_config() < 0) return -1;
    int result = unregister_user(sig);
    unlock_config();
    return result;
}

void imre_fault_unregister_all(void) {
    if (lock_config() < 0) return;
    for (int sig = 1; sig < NSIG && user_count; sig++) unregister_user(sig);
    unlock_config();
}

int imre_fault_enable(int fd, int all_threads, int c_stack) {
    if (fd < 0) { errno = EINVAL; return -1; }
    if (lock_config() < 0) return -1;
    unsigned long long opts = (uint32_t)fd | ((unsigned long long)!!all_threads << 32)
                                          | ((unsigned long long)!!c_stack << 33);
    unsigned installed = atomic_load(&fatal_mask);
    if (installed == ALL_FATAL) {
        atomic_store(&settings, opts);
        unlock_config();
        return 0;
    }
    Dispositions *old = atomic_load(&originals);
    Dispositions *d = malloc(sizeof(*d));
    if (!d) goto fail;
    for (size_t i = 0; i < NSIGNALS; i++) {
        if (installed & (1u << i)) d->saved[i] = old->saved[i];
        else if (sigaction(signals[i].signum, NULL, &d->saved[i]) < 0) { free(d); goto fail; }
    }
    begin_transfer(-1, old, d);
    alternate_stack();
    atomic_store(&originals, d);
    while (atomic_load(&readers)) {}
    atomic_store(&settings, opts);
    struct sigaction action = { .sa_sigaction = fatal, .sa_flags = SA_SIGINFO | SA_ONSTACK | SA_NODEFER };
    sigemptyset(&action.sa_mask);
    sigaddset(&action.sa_mask, SIGPIPE);
    for (size_t i = 0; i < NSIGNALS; i++) {
        if (installed & (1u << i)) continue;
        if (sigaction(signals[i].signum, &action, NULL) != 0) {
            int error = errno;
            while (i) {
                i--;
                if (!(installed & (1u << i))) sigaction(signals[i].signum, &d->saved[i], NULL);
            }
            atomic_store(&transfer.signal, 0);
            free(old);
            errno = error;
            goto fail;
        }
    }
    atomic_store(&fatal_mask, ALL_FATAL);
    update_publication();
    atomic_store(&transfer.signal, 0);
    free(old);
    unlock_config();
    return 0;
fail:
    unlock_config();
    return -1;
}
int imre_fault_disable(void) {
    if (lock_config() < 0) return 0;
    unsigned installed = atomic_load(&fatal_mask);
    if (installed) {
        Dispositions *d = atomic_load(&originals);
        begin_transfer(-1, d, d);
        for (size_t i = 0; i < NSIGNALS; i++) {
            if (installed & (1u << i)) sigaction(signals[i].signum, &d->saved[i], NULL);
        }
        atomic_store(&fatal_mask, 0);
        update_publication();
        atomic_store(&transfer.signal, 0);
    }
    unlock_config();
    return installed != 0;
}
#else
/* Native signal management is implemented for Imre's Linux and macOS hosts. */
int imre_fault_enable(int fd, int all, int stack) { (void)fd;(void)all;(void)stack;errno=ENOSYS;return -1; }
int imre_fault_disable(void) { return 0; }
int imre_fault_enabled(void) { return 0; }
uint32_t imre_fault_generation(void) { return 0; }
void imre_fault_frame(size_t i,const char*f,size_t fl,const char*n,size_t nl,uint32_t l)
    { (void)i;(void)f;(void)fl;(void)n;(void)nl;(void)l; }
void imre_fault_line(size_t i,uint32_t l) { (void)i;(void)l; }
void imre_fault_truncate(size_t d) { (void)d; }
void imre_fault_dump(int fd,int all) { (void)fd;(void)all; }
#endif
