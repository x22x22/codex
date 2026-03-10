// Bazel's Linux linker setup does not currently resolve V8's
// `__libc_stack_end` reference from the system runtime. Provide a weak
// definition and initialize it from the current thread's stack bounds so the
// prebuilt archive can link and main-thread stack probing still has a sensible
// fallback.
#include <pthread.h>
#include <stddef.h>
#include <stdint.h>

extern "C" __attribute__((weak)) void* __libc_stack_end = nullptr;

__attribute__((constructor)) static void init_libc_stack_end() {
  pthread_attr_t attr;
  if (pthread_getattr_np(pthread_self(), &attr) != 0) {
    return;
  }

  void* base = nullptr;
  size_t size = 0;
  if (pthread_attr_getstack(&attr, &base, &size) == 0 && base != nullptr) {
    __libc_stack_end = static_cast<uint8_t*>(base) + size;
  }

  pthread_attr_destroy(&attr);
}
