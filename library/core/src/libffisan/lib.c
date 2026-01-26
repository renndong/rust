#define _GNU_SOURCE
#define __FFISAN_INNER__
#include "ffisan.h"
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static pthread_once_t init_once = PTHREAD_ONCE_INIT;
static void *(*libc_malloc)(size_t) = NULL;
static void (*libc_free)(void *) = NULL;
static void *(*libc_realloc)(void *, size_t) = NULL;

#define DEBUG
#ifdef DEBUG
#include <stdatomic.h>

static atomic_int c_alloc_count = 0, c_free_count = 0;
static atomic_int r_alloc_count = 0, r_free_count = 0;

int safe_log(const char *restrict format, ...) {
  char buffer[256];
  va_list args;

  va_start(args, format);
  int len = vsnprintf(buffer, sizeof(buffer), format, args);
  va_end(args);

  fprintf(stderr, "%s", buffer);
  fflush(stderr);
  return len;
}

#else

#define safe_log(...)
#define atomic_fetch_add(...)

#endif

/************************** free_ring_t definition ****************************/

static free_ring_t free_ring_queue;

static void free_ring_init(free_ring_t *fr) {
  fr->head = fr->tail = 0;
  pthread_mutex_init(&fr->mutex, NULL);
}

static inline int free_ring_full(free_ring_t *fr) {
  return (fr->tail + 1) % RING_SIZE == fr->head;
}

static inline int free_ring_empty(free_ring_t *fr) {
  return fr->head == fr->tail;
}

static inline void *free_ring_pop(free_ring_t *fr) {
  void *ptr = fr->ring[fr->head];
  fr->head = (fr->head + 1) % RING_SIZE;

  return ptr;
}

static inline void free_ring_push(free_ring_t *fr, void *ptr) {
  fr->ring[fr->tail] = ptr;
  fr->tail = (fr->tail + 1) % RING_SIZE;
}

static inline void *free_ring_pop_when_full(free_ring_t *fr) {
  if (free_ring_full(fr)) {
    return free_ring_pop(fr);
  }
  return NULL;
}

static inline int free_ring_count(free_ring_t *fr) {
  return (fr->tail - fr->head + RING_SIZE) % RING_SIZE;
}

static void *free_ring_push_and_pop(free_ring_t *fr, void *ptr) {
  void *old;

  pthread_mutex_lock(&fr->mutex);
  old = free_ring_pop_when_full(fr);
  free_ring_push(fr, ptr);
  pthread_mutex_unlock(&fr->mutex);

  return old;
}

/*************************** allocator definition *****************************/

static void setup_internal(void) {
#ifdef DEBUG
  atomic_store(&c_alloc_count, 0);
  atomic_store(&r_alloc_count, 0);
  atomic_store(&c_free_count, 0);
  atomic_store(&r_free_count, 0);
#endif
  free_ring_init(&free_ring_queue);

  libc_malloc = dlsym(RTLD_NEXT, "malloc");
  libc_free = dlsym(RTLD_NEXT, "free");
  libc_realloc = dlsym(RTLD_NEXT, "realloc");
}

static void fill_red_zone(void *ptr) {
  unsigned *p = (unsigned *)ptr;
  int len = RED_ZONE_SIZE / sizeof(unsigned);
  for (int i = 0; i < len; i++)
    p[i] = RED_ZONE_PATTERN;
}

static inline header_t *get_header(void *ptr) {
  header_t *header =
      (header_t *)((uintptr_t)ptr - sizeof(header_t) - RED_ZONE_SIZE);
  return header->header_magic == HEADER_MAGIC ? header : NULL;
}

static void *get_raw_ptr(header_t *header) {
  uintptr_t data;

  data = (uintptr_t)header + sizeof(header_t) + RED_ZONE_SIZE;
  return (void *)(data - header->offset);
}

static size_t calc_alloc_size(size_t size, size_t align) {
  return size + sizeof(header_t) + RED_ZONE_SIZE * 2 + (align - 1);
}

static void *align_to(void *ptr, size_t align) {
  size_t mask = align - 1;
  return (void *)(((uintptr_t)ptr + mask) & ~mask);
}

static void *fill_metadata_and_take_data_ptr(void *ptr, size_t size,
                                             size_t align,
                                             union header_flag flag) {
  char *data, *first_red_zone, *second_red_zone;
  header_t *header;

  data = (char *)align_to(
      (void *)((uintptr_t)ptr + sizeof(header_t) + RED_ZONE_SIZE), align);
  first_red_zone = data - RED_ZONE_SIZE;
  second_red_zone = data + size;

  fill_red_zone(first_red_zone);
  fill_red_zone(second_red_zone);

  header = (header_t *)(first_red_zone - sizeof(header_t));
  header->data_size = size;
  header->offset = (uintptr_t)data - (uintptr_t)ptr;
  header->header_magic = HEADER_MAGIC;
  header->flag.data = flag.data;

  safe_log("++ size %d raw %p  header %p fz %p data %p sz %p\n", size, ptr,
           header, first_red_zone, data, second_red_zone);

  return data;
}

static void *malloc_impl(size_t size, size_t align, unsigned f_alloc) {
  pthread_once(&init_once, setup_internal);

  size_t alloc_size;
  void *ptr;
  union header_flag flag;

  safe_log("libmalloc %p\n", libc_malloc);

  alloc_size = calc_alloc_size(size, align);
  ptr = libc_malloc(alloc_size);
  flag.field.f_alloc = f_alloc;

  return fill_metadata_and_take_data_ptr(ptr, size, align, flag);
}

static int free_impl(void *data, unsigned f_free) {
  pthread_once(&init_once, setup_internal);

  void *raw, *old;
  header_t *header;

  if (!data)
    return 0;

  header = get_header(data);
  if (!header) { // not alloc by us
    safe_log("header is nil %p\n", data);
    libc_free(data);
    return 0;
  }

  raw = get_raw_ptr(header);
  header->flag.field.f_free = f_free;

  safe_log("-- size %d raw %p header %p data %p\n", header->data_size, raw,
           header, data);

  // libc_free(raw);
  // return 1;

  old = free_ring_push_and_pop(&free_ring_queue, raw);
  if (old) {
    safe_log("realfree %p\n", old);
    libc_free(old);
    return 1;
  }
  return 0;
}

static void *realloc_impl(void *ptr, size_t size, unsigned f_realloc) {
  void *new_data;
  header_t *header, *new_header;
  unsigned f_alloc, f_free;

  safe_log("realloc size %d ptr %p\n", size, ptr);

  if (!ptr) {
    return malloc(size);
  }
  if (!size) {
    free(ptr);
    return NULL;
  }

  header = get_header(ptr);
  if (!header) {
    safe_log("realloc no header %p\n", ptr);
    return libc_realloc(ptr, size);
  }

  if (f_realloc == F_REALLOC_R) {
    f_alloc = F_ALLOC_R;
    f_free = F_FREE_R;
  } else {
    f_alloc = F_ALLOC_C;
    f_free = F_FREE_C;
  }

  new_data = malloc_impl(size, DEFAULT_ALIGN, f_alloc);
  new_header = get_header(new_data);

  memcpy(new_data, ptr, size < header->data_size ? size : header->data_size);
  new_header->flag.data = header->flag.data;
  new_header->flag.field.f_realloc = f_realloc;

  free_impl(ptr, f_free);
  return new_data;
}

static void *calloc_impl(size_t nitems, size_t size, unsigned f_alloc) {
  size_t real_size = nitems * size;
  void *data;

  data = malloc_impl(real_size, DEFAULT_ALIGN, f_alloc);
  memset(data, 0, real_size);
  atomic_fetch_add(&c_alloc_count, 1);

  return data;
}

static int posix_memalign_impl(void **memptr, size_t alignment, size_t size,
                               unsigned f_alloc) {
  if ((alignment & (alignment - 1)) != 0 || alignment == 0) {
    return EINVAL;
  }

  void *ptr = malloc_impl(size, alignment, f_alloc);
  if (!ptr) {
    return ENOMEM;
  }

  *memptr = ptr;
  return 0;
}

void *malloc(size_t size) {
  void *data;

  data = malloc_impl(size, DEFAULT_ALIGN, F_ALLOC_C);
  atomic_fetch_add(&c_alloc_count, 1);
  return data;
}

void free(void *ptr) {
  if (free_impl(ptr, F_FREE_C)) {
    atomic_fetch_add(&c_free_count, 1);
  }
}

void *realloc(void *ptr, size_t size) {
  return realloc_impl(ptr, size, F_REALLOC_C);
}

void *calloc(size_t nitems, size_t size) {
  return calloc_impl(nitems, size, F_ALLOC_C);
}

int posix_memalign(void **memptr, size_t alignment, size_t size) {
  return posix_memalign_impl(memptr, alignment, size, F_ALLOC_C);
}

void *aligned_alloc(size_t alignment, size_t size) {
  pthread_once(&init_once, setup_internal);

  if ((size & (alignment - 1)) != 0) {
    errno = EINVAL;
    return NULL;
  }
  return malloc_impl(size, alignment, F_ALLOC_C);
}

void *memalign(size_t alignment, size_t size) {
  pthread_once(&init_once, setup_internal);

  void *ptr;
  if (posix_memalign(&ptr, alignment, size) != 0) {
    return NULL;
  }
  return ptr;
}

void *__ffi_sanitizer_rust_alloc(size_t size) {
  void *data;

  data = malloc_impl(size, DEFAULT_ALIGN, F_ALLOC_R);
  atomic_fetch_add(&r_alloc_count, 1);
  return data;
}

void __ffi_sanitizer_rust_free(void *ptr) {
  if (free_impl(ptr, F_FREE_R)) {
    atomic_fetch_add(&r_free_count, 1);
  }
}

void *__ffi_sanitizer_rust_realloc(void *ptr, size_t size) {
  return realloc_impl(ptr, size, F_REALLOC_R);
}

void *__ffi_sanitizer_rust_calloc(size_t nitems, size_t size) {
  return calloc_impl(nitems, size, F_ALLOC_R);
}

int __ffi_sanitizer_rust_posix_memalign(void **memptr, size_t alignment,
                                        size_t size) {
  return posix_memalign_impl(memptr, alignment, size, F_ALLOC_R);
}

header_t *__ffi_sanitizer_get_header(void *ptr) { return get_header(ptr); }

__attribute__((destructor)) void cleanup(void) {
#ifdef DEBUG
  int buffered = free_ring_count(&free_ring_queue);
  int alloc_count = atomic_load(&c_alloc_count) + atomic_load(&r_alloc_count);
  int free_count = atomic_load(&c_free_count) + atomic_load(&r_free_count);
  safe_log("alloc : %d free: %d real free: %d buffered: %d\n", alloc_count,
           free_count + buffered, free_count, buffered);
#endif
}