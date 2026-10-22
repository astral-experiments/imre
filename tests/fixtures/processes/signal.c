#include <signal.h>

int main(void) {
    raise(SIGPIPE);
    return 77;
}
