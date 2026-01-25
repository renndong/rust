
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>

static int alloc_count, free_count;

int safe_log(const char *restrict format, ...) {
  char buffer[256];
  va_list args;

  va_start(args, format);
  int len = vsnprintf(buffer, sizeof(buffer), format, args);
  va_end(args);

  fprintf(stderr, "%s", buffer);
  return len;
}

void *malloc(size_t size) {
  void *(*libc_malloc)(size_t) = dlsym(RTLD_NEXT, "malloc");
  safe_log("malloc(%zu)\n", size);
  alloc_count++;
  return libc_malloc(size);
}

void free(void *ptr) {
  void (*libc_free)(void *) = dlsym(RTLD_NEXT, "free");
  safe_log("free(%p)\n", ptr);
  free_count++;
  libc_free(ptr);
}

void *__ffi_sanitizer_rust_alloc(size_t size) {
  void *(*libc_malloc)(size_t) = dlsym(RTLD_NEXT, "malloc");
  safe_log("rust malloc(%zu)\n", size);
  alloc_count++;
  return libc_malloc(size);
}

void __ffi_sanitizer_rust_free(void *ptr) {
  void (*libc_free)(void *) = dlsym(RTLD_NEXT, "free");
  safe_log("rust free(%p)\n", ptr);
  free_count++;
  libc_free(ptr);
}

int __ffi_sanitizer_get_metadata(int *p) { return 0; }

__attribute__((constructor)) void setup() {
  alloc_count = 0;
  free_count = 0;
  fprintf(stderr, "setup: alloc : %d free: %d\n", alloc_count, free_count);
}

__attribute__((destructor)) void cleanup() {
  safe_log("alloc : %d free: %d\n", alloc_count, free_count);
}