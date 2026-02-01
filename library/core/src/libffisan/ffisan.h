#ifndef __FFISAN_H__
#define __FFISAN_H__

#include <stddef.h>
#include <stdint.h>

#define RED_ZONE_SIZE 16
#define RED_ZONE_PATTERN 0xDEADBEEF
#define HEADER_MAGIC 0xABC

#define DEFAULT_ALIGN 16

#define F_ALLOC_R 0b01
#define F_ALLOC_C 0b10
#define F_FREE_R 0b01
#define F_FREE_C 0b10
#define F_OWNED 0b1

typedef struct header {
  unsigned int data_size;
  unsigned int offset;
  struct {
    uintptr_t alloc_list : 48;
    unsigned magic : 12;
    unsigned f_alloc : 2;
    unsigned f_free : 2;
  } cps;
} header_t;

// for c/c++ program
void *malloc(size_t size);
void free(void *ptr);
void *realloc(void *ptr, size_t size);
void *calloc(size_t nitems, size_t size);
int posix_memalign(void **memptr, size_t alignment, size_t size);
void *aligned_alloc(size_t alignment, size_t size);
void *memalign(size_t alignment, size_t size);

// for rust allocator
void *__ffi_sanitizer_rust_alloc(size_t size);
void __ffi_sanitizer_rust_free(void *ptr);
void *__ffi_sanitizer_rust_realloc(void *ptr, size_t size);
void *__ffi_sanitizer_rust_calloc(size_t nitems, size_t size);
int __ffi_sanitizer_rust_posix_memalign(void **memptr, size_t alignment,
                                        size_t size);

header_t *__ffi_sanitizer_get_header(void *ptr);
int __ffi_sanitizer_check_red_zone(void *ptr);
int __ffi_sanitizer_print_leak_summary();
void __ffi_sanitizer_put_alloc_list(void *data, char *file, unsigned len, unsigned line);

#ifdef __FFISAN_INNER__
// the following define is only available in ffisan library, and
// not expose to rust bindings
#include <pthread.h>

#define RING_SIZE 1024

typedef struct free_ring {
  void *ring[RING_SIZE];
  size_t head;
  size_t tail;
  pthread_mutex_t mutex;
} free_ring_t;

typedef struct list_node {
  struct list_node *prev;
  struct list_node *next;
  char *last_file;
  unsigned len;
  unsigned last_line;
  void *self;
} list_node_t;

typedef struct list_head {
  list_node_t head;
  pthread_mutex_t mutex;
} list_head_t;

#endif

#endif