/* Drive the concurrent_vector base through its exported ABI the way the inline
 * code in Microsoft's <concurrent_vector.h> does, and check every element
 * address the runtime hands out, derives or passes to a callback.
 *
 * The header never asks the runtime where element i is: operator[] reads
 * segment[k] from the table and adds (i - base(k)) * sizeof(T) itself, and it
 * gives the allocator callback a count of elements. So the table has to agree
 * with that arithmetic, every block has to be large enough for the elements
 * placed in it, and every callback has to be given exactly the elements it is
 * meant to construct, assign or destroy -- never one past the end, never one
 * twice. A shadow model of each vector records which elements are alive.
 *
 * Nothing is written through an address before it has been checked against the
 * blocks the allocator returned, so a broken runtime reports failures instead
 * of corrupting the heap and crashing somewhere unrelated.
 *
 *   concrt_vector_segments.exe [path\to\concrt140.dll]
 *
 * The same binary runs against Microsoft's concrt140.dll, which is how the
 * allocator's unit (elements, not bytes) and the expected layout were settled:
 * versions 14.16, 14.40, 14.44 and 14.51 all pass. It is built for x86_64
 * only; the i386 exports share the code but are not exercised here.
 *
 * Run by tests/run-concrt-tests.sh. */

/* For %zu, which the msvcrt printf of older Windows does not know. */
#define __USE_MINGW_ANSI_STDIO 1
#include <windows.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct vector
{
    void *(__cdecl *allocator)(struct vector *, size_t);
    void *storage[3];
    size_t first_block;
    size_t early_size;
    void **segment;
};

typedef void (__cdecl *array_fn)(void *, const void *, size_t);
typedef void (__cdecl *clear_fn)(void *, size_t);

static void (__cdecl *p_dtor)(struct vector *);
static size_t (__cdecl *p_capacity)(const struct vector *);
static void (__cdecl *p_reserve)(struct vector *, size_t, size_t, size_t);
static size_t (__cdecl *p_clear)(struct vector *, clear_fn);
static void (__cdecl *p_copy)(struct vector *, const struct vector *, size_t, array_fn);
static void (__cdecl *p_assign)(struct vector *, const struct vector *, size_t, clear_fn, array_fn, array_fn);
static size_t (__cdecl *p_grow_by)(struct vector *, size_t, size_t, array_fn, const void *);
static size_t (__cdecl *p_grow_to_at_least)(struct vector *, size_t, size_t, array_fn, const void *);
static void *(__cdecl *p_push_back)(struct vector *, size_t, size_t *);
static void (__cdecl *p_resize)(struct vector *, size_t, size_t, size_t, clear_fn, array_fn, const void *);

#define MAX_ELEMS 256
#define MAX_BLOCKS 64
#define GUARD 64
#define GUARD_BYTE 0xa5
#define MARKER ((void *)1)

struct block { BYTE *ptr; size_t count; struct vector *owner; };
struct model
{
    const char *name;
    struct vector v;
    BOOL live[MAX_ELEMS];
    unsigned int tag[MAX_ELEMS];
};

static struct block blocks[MAX_BLOCKS];
static int nblocks;
static struct model models[2];
static size_t es;               /* element size of the vectors under test */
static unsigned int fill_tag;   /* the value grow_by and resize replicate */
static char context[160];
static int failures, checks;

#define FAIL(...) do { printf("FAIL [%s] ", context); printf(__VA_ARGS__); printf("\n"); failures++; } while (0)
#define CHECK(c, ...) do { checks++; if (!(c)) FAIL(__VA_ARGS__); } while (0)

static size_t segment_index_of(size_t i)
{
    size_t k = 0;
    i |= 1;
    while (i >>= 1) k++;
    return k;
}

static size_t segment_base(size_t k) { return ((size_t)1 << k) & ~(size_t)1; }
static size_t table_size(const struct vector *v) { return v->segment == v->storage ? 3 : sizeof(void *) * 8; }

/* Where operator[] in the header looks for element i, or NULL when the segment
 * holding it does not exist yet. */
static BYTE *element_address(const struct vector *v, size_t i)
{
    size_t k = segment_index_of(i);
    void *seg;

    if (k >= table_size(v)) return NULL;
    seg = v->segment[k];
    if (!seg || seg == MARKER) return NULL;
    return (BYTE *)seg + es * (i - segment_base(k));
}

/* Is [p, p + n elements) inside one block the allocator handed out? */
static BOOL in_block(const BYTE *p, size_t n)
{
    int b;

    for (b = 0; b < nblocks; b++)
        if (p >= blocks[b].ptr && p + n * es <= blocks[b].ptr + blocks[b].count * es) return TRUE;
    return FALSE;
}

static BOOL locate(const void *addr, struct model **m, size_t *idx)
{
    size_t i, n;

    for (n = 0; n < 2; n++)
    {
        if (!models[n].name) continue;
        for (i = 0; i < MAX_ELEMS; i++)
        {
            if (element_address(&models[n].v, i) == addr)
            {
                *m = &models[n];
                *idx = i;
                return TRUE;
            }
        }
    }
    return FALSE;
}

static void put(BYTE *p, unsigned int tag)
{
    memset(p, tag & 0xff, es);
    memcpy(p, &tag, es < sizeof(tag) ? es : sizeof(tag));
}

static BOOL holds(const BYTE *p, unsigned int tag)
{
    BYTE expect[64];

    put(expect, tag);
    return !memcmp(p, expect, es);
}

static void *__cdecl allocate(struct vector *v, size_t count)
{
    BYTE *p;

    if (nblocks == MAX_BLOCKS || count > MAX_ELEMS * 64)
    {
        printf("FAIL [%s] allocator asked for %zu elements (block %d); giving up\n", context, count, nblocks);
        exit(1);
    }
    p = malloc(count * es + GUARD);
    memset(p, 0xcd, count * es);
    memset(p + count * es, GUARD_BYTE, GUARD);
    blocks[nblocks].ptr = p;
    blocks[nblocks].count = count;
    blocks[nblocks].owner = v;
    nblocks++;
    return p;
}

/* Find the model element a callback was given, or report what it was given. */
static struct model *target(const BYTE *p, size_t *idx, const char *what, size_t j)
{
    struct model *m;

    if (!locate(p, &m, idx))
    {
        FAIL("%s: element %zu of the range is not an element of any vector", what, j);
        return NULL;
    }
    if (!in_block(p, 1))
    {
        FAIL("%s: %s[%zu] lies outside every allocated block", what, m->name, *idx);
        return NULL;
    }
    return m;
}

static BOOL source(const BYTE *p, unsigned int *tag, const char *what, size_t j)
{
    struct model *m;
    size_t i;

    if (!(m = target(p, &i, what, j))) return FALSE;
    if (!m->live[i])
    {
        FAIL("%s: reads %s[%zu], which is not alive", what, m->name, i);
        return FALSE;
    }
    if (!holds(p, m->tag[i])) FAIL("%s: %s[%zu] does not hold its value", what, m->name, i);
    *tag = m->tag[i];
    return TRUE;
}

static void construct(BYTE *d, unsigned int tag, const char *what, size_t j)
{
    struct model *m;
    size_t i;

    if (!(m = target(d, &i, what, j))) return;
    if (m->live[i]) FAIL("%s: constructs %s[%zu] again while it is alive", what, m->name, i);
    put(d, tag);
    m->live[i] = TRUE;
    m->tag[i] = tag;
}

/* _Copy_array: copy-construct n elements from n source elements. */
static void __cdecl copy_array(void *dst, const void *src, size_t n)
{
    unsigned int tag;
    size_t j;

    for (j = 0; j < n; j++)
        if (source((const BYTE *)src + j * es, &tag, "copy", j))
            construct((BYTE *)dst + j * es, tag, "copy", j);
}

/* _Initialize_array_by: construct n copies of one value. */
static void __cdecl fill_array(void *dst, const void *value, size_t n)
{
    size_t j;

    CHECK(holds(value, fill_tag), "fill: was not given the caller's value");
    for (j = 0; j < n; j++) construct((BYTE *)dst + j * es, fill_tag, "fill", j);
}

/* _Assign_array: assign n live elements from n source elements. */
static void __cdecl assign_array(void *dst, const void *src, size_t n)
{
    struct model *m;
    unsigned int tag;
    size_t j, i;

    for (j = 0; j < n; j++)
    {
        BYTE *d = (BYTE *)dst + j * es;
        if (!source((const BYTE *)src + j * es, &tag, "assign", j)) continue;
        if (!(m = target(d, &i, "assign", j))) continue;
        if (!m->live[i]) FAIL("assign: assigns to %s[%zu], which is not alive", m->name, i);
        put(d, tag);
        m->tag[i] = tag;
    }
}

/* _Destroy_array: destroy n live elements. */
static void __cdecl clear_array(void *dst, size_t n)
{
    struct model *m;
    size_t j, i;

    for (j = 0; j < n; j++)
    {
        BYTE *d = (BYTE *)dst + j * es;
        if (!(m = target(d, &i, "destroy", j))) continue;
        if (!m->live[i]) FAIL("destroy: destroys %s[%zu], which is not alive", m->name, i);
        else if (!holds(d, m->tag[i])) FAIL("destroy: %s[%zu] was overwritten while alive", m->name, i);
        memset(d, 0xdd, es);
        m->live[i] = FALSE;
    }
}

static struct model *open_vector(int n, const char *name)
{
    struct model *m = &models[n];

    memset(m, 0, sizeof(*m));
    m->name = name;
    m->v.allocator = allocate;
    m->v.segment = m->v.storage;
    return m;
}

/* Everything the header relies on: alive exactly below early_size, every alive
 * element where operator[] will look for it, inside a block and still holding
 * its value; no block overrun; capacity made of exactly what was allocated. */
static void verify(struct model *m)
{
    size_t i, allocated = 0, capacity;
    int b, g;

    for (i = 0; i < MAX_ELEMS; i++)
    {
        BYTE *p;
        if (m->live[i] != (i < m->v.early_size))
        {
            FAIL("%s[%zu] is %s but the size is %zu", m->name, i, m->live[i] ? "alive" : "not alive", m->v.early_size);
            continue;
        }
        if (!m->live[i]) continue;
        p = element_address(&m->v, i);
        if (!p || !in_block(p, 1)) FAIL("%s[%zu] lies outside every allocated block", m->name, i);
        else if (!holds(p, m->tag[i])) FAIL("%s[%zu] does not hold its value", m->name, i);
    }
    for (b = 0; b < nblocks; b++)
    {
        for (g = 0; g < GUARD; g++)
            if (blocks[b].ptr[blocks[b].count * es + g] != GUARD_BYTE)
            {
                FAIL("the bytes after block %d (%zu elements) were overwritten", b, blocks[b].count);
                blocks[b].ptr[blocks[b].count * es + g] = GUARD_BYTE;
            }
        if (blocks[b].owner == &m->v) allocated += blocks[b].count;
    }
    /* Microsoft's runtime sometimes allocates a first block twice the capacity
     * it then reports (copying four elements into an empty vector allocates
     * eight), so the bound is loose; an allocator handed bytes instead of
     * elements overshoots it by the element size. */
    capacity = p_capacity(&m->v);
    CHECK(capacity <= allocated && allocated <= 2 * capacity,
          "%s: capacity %zu but %zu elements were allocated", m->name, capacity, allocated);
}

/* What operator[] does: write every element through the table. */
static void write_all(struct model *m, unsigned int base)
{
    size_t i;

    for (i = 0; i < m->v.early_size; i++)
    {
        BYTE *p = element_address(&m->v, i);
        if (!p || !in_block(p, 1))
        {
            FAIL("operator[]: %s[%zu] lies outside every allocated block", m->name, i);
            continue;
        }
        put(p, base + (unsigned int)i);
        m->tag[i] = base + (unsigned int)i;
    }
}

static void push_back(struct model *m, unsigned int tag)
{
    size_t idx = ~(size_t)0;
    BYTE *p = p_push_back(&m->v, es, &idx);

    CHECK(idx < MAX_ELEMS, "push_back: index %zu", idx);
    if (idx >= MAX_ELEMS) return;
    CHECK(p == element_address(&m->v, idx), "push_back: %s[%zu] is not where operator[] finds it", m->name, idx);
    if (!in_block(p, 1))
    {
        FAIL("push_back: %s[%zu] lies outside every allocated block", m->name, idx);
        return;
    }
    if (m->live[idx]) FAIL("push_back: %s[%zu] was already alive", m->name, idx);
    put(p, tag);
    m->live[idx] = TRUE;
    m->tag[idx] = tag;
}

static void grow_by(struct model *m, size_t n, unsigned int tag)
{
    BYTE value[64];
    size_t before = m->v.early_size;

    fill_tag = tag;
    put(value, tag);
    CHECK(p_grow_by(&m->v, n, es, fill_array, value) == before, "grow_by returns the old size");
}

static void grow_to_at_least(struct model *m, size_t n, unsigned int tag)
{
    BYTE value[64];
    size_t before = m->v.early_size;

    fill_tag = tag;
    put(value, tag);
    CHECK(p_grow_to_at_least(&m->v, n, es, fill_array, value) == before, "grow_to_at_least returns the old size");
}

/* Build a vector of n elements the way most code does: push_back, so the first
 * block is the smallest one and the segments are separate allocations. */
static void fill_by_push(struct model *m, size_t n, unsigned int base)
{
    size_t i;

    for (i = 0; i < n; i++) push_back(m, base + (unsigned int)i);
}

static void close_vector(struct model *m)
{
    int b, kept = 0;
    size_t i;

    p_clear(&m->v, clear_array);
    for (i = 0; i < MAX_ELEMS; i++)
        if (m->live[i]) { FAIL("%s[%zu] outlived clear", m->name, i); break; }
    for (b = 0; b < nblocks; b++)
    {
        if (blocks[b].owner == &m->v) free(blocks[b].ptr);
        else blocks[kept++] = blocks[b];
    }
    nblocks = kept;
    p_dtor(&m->v);
    m->name = NULL;
}

static void begin(const char *fmt, ...)
{
    va_list args;

    va_start(args, fmt);
    _vsnprintf(context, sizeof(context) - 1, fmt, args);
    va_end(args);
}

/* The report: a first reserve(64) or grow_by(64) of a small element puts the
 * first six segments in one block, and elements 32..63 must stay inside it. */
static void test_first_block(void)
{
    struct model *a;

    begin("es=%zu reserve(64) then grow_by(64)", es);
    a = open_vector(0, "v");
    p_reserve(&a->v, 64, es, ~(size_t)0 / es);
    CHECK(a->v.first_block == 6, "first_block %zu after reserve(64)", a->v.first_block);
    CHECK(nblocks == 1 && blocks[0].count == 64,
          "reserve(64) allocates one block of 64 elements, not %zu (in %d blocks)", nblocks ? blocks[0].count : 0, nblocks);
    grow_by(a, 64, 0x1000);
    verify(a);
    write_all(a, 0x2000);
    verify(a);
    close_vector(a);

    begin("es=%zu grow_by(64) on an empty vector", es);
    a = open_vector(0, "v");
    grow_by(a, 64, 0x3000);
    /* Microsoft's runtime makes this first block 128 elements, not 64. */
    CHECK(nblocks == 1 && blocks[0].count >= 64 && blocks[0].count <= 128,
          "grow_by(64) allocates one block of 64 to 128 elements, not %zu (in %d blocks)", nblocks ? blocks[0].count : 0, nblocks);
    verify(a);
    write_all(a, 0x4000);
    grow_by(a, 30, 0x5000);
    verify(a);
    close_vector(a);
}

/* grow_by and grow_to_at_least appending into a partly used segment. */
static void test_append(void)
{
    static const size_t starts[] = { 1, 2, 3, 5, 6, 9, 17 }, adds[] = { 1, 2, 3, 7, 20 };
    struct model *a;
    size_t s, n;

    for (s = 0; s < ARRAYSIZE(starts); s++)
        for (n = 0; n < ARRAYSIZE(adds); n++)
        {
            begin("es=%zu size %zu grow_by(%zu)", es, starts[s], adds[n]);
            a = open_vector(0, "v");
            fill_by_push(a, starts[s], 0x100);
            grow_by(a, adds[n], 0x6000 + (unsigned int)n);
            verify(a);
            fill_by_push(a, 3, 0x200);
            verify(a);
            close_vector(a);

            begin("es=%zu size %zu grow_to_at_least(%zu)", es, starts[s], starts[s] + adds[n]);
            a = open_vector(0, "v");
            fill_by_push(a, starts[s], 0x300);
            grow_to_at_least(a, starts[s] + adds[n], 0x7000 + (unsigned int)n);
            verify(a);
            grow_to_at_least(a, starts[s], 0x7100);
            verify(a);
            close_vector(a);
        }
}

/* assign between vectors of every relative size, including ones whose last
 * elements share a segment: only the elements that exist may be touched. */
static void test_assign(void)
{
    static const size_t sizes[] = { 0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 13, 20, 33 };
    struct model *a, *b;
    size_t x, y, i;

    for (x = 0; x < ARRAYSIZE(sizes); x++)
        for (y = 0; y < ARRAYSIZE(sizes); y++)
        {
            begin("es=%zu assign size %zu = size %zu", es, sizes[x], sizes[y]);
            a = open_vector(0, "a");
            b = open_vector(1, "b");
            fill_by_push(a, sizes[x], 0x8000);
            fill_by_push(b, sizes[y], 0x9000);
            p_assign(&a->v, &b->v, es, clear_array, assign_array, copy_array);
            CHECK(a->v.early_size == sizes[y], "size %zu after assign", a->v.early_size);
            verify(a);
            verify(b);
            for (i = 0; i < sizes[y] && i < MAX_ELEMS; i++)
                if (a->live[i] && a->tag[i] != b->tag[i]) { FAIL("a[%zu] is not b[%zu] after assign", i, i); break; }
            close_vector(b);
            close_vector(a);
        }

    begin("es=%zu copy into an empty vector", es);
    a = open_vector(0, "a");
    b = open_vector(1, "b");
    fill_by_push(b, 21, 0xa000);
    p_copy(&a->v, &b->v, es, copy_array);
    verify(a);
    verify(b);
    close_vector(b);
    close_vector(a);
}

/* resize down within one segment, across segments and to zero, and up. */
static void test_resize(void)
{
    static const size_t sizes[] = { 1, 2, 3, 4, 5, 7, 8, 9, 10, 16, 20, 33 };
    static const size_t targets[] = { 0, 1, 2, 3, 4, 5, 6, 8, 9, 16, 17, 40 };
    BYTE value[64];
    struct model *a;
    size_t s, t;

    for (s = 0; s < ARRAYSIZE(sizes); s++)
        for (t = 0; t < ARRAYSIZE(targets); t++)
        {
            begin("es=%zu resize %zu to %zu", es, sizes[s], targets[t]);
            a = open_vector(0, "v");
            fill_by_push(a, sizes[s], 0xb000);
            fill_tag = 0xc000;
            put(value, fill_tag);
            p_resize(&a->v, targets[t], es, ~(size_t)0 / es, clear_array, fill_array, value);
            CHECK(a->v.early_size == targets[t], "size %zu after resize", a->v.early_size);
            verify(a);
            close_vector(a);
        }
}

static BOOL is_builtin(HMODULE module)
{
    return !memcmp((const char *)module + 0x40, "Wine builtin DLL", 16);
}

int main(int argc, char **argv)
{
    static const size_t sizes[] = { 1, 2, 4, 8, 12, 16, 24 };
    HMODULE dll = LoadLibraryA(argc > 1 ? argv[1] : "concrt140.dll");
    char path[MAX_PATH];
    size_t n;

#define LOAD(target, symbol) do { target = (void *)GetProcAddress(dll, symbol); if (!target) { printf("Missing: %s\n", symbol); return 100; } } while (0)
    if (!dll)
    {
        printf("could not load %s (%lu)\n", argc > 1 ? argv[1] : "concrt140.dll", GetLastError());
        return 100;
    }
    GetModuleFileNameA(dll, path, sizeof(path));
    printf("testing %s (%s)\n", path, is_builtin(dll) ? "Wine builtin" : "native");
    LOAD(p_dtor, "??1_Concurrent_vector_base_v4@details@Concurrency@@IEAA@XZ");
    LOAD(p_capacity, "?_Internal_capacity@_Concurrent_vector_base_v4@details@Concurrency@@IEBA_KXZ");
    LOAD(p_reserve, "?_Internal_reserve@_Concurrent_vector_base_v4@details@Concurrency@@IEAAX_K00@Z");
    LOAD(p_clear, "?_Internal_clear@_Concurrent_vector_base_v4@details@Concurrency@@IEAA_KP6AXPEAX_K@Z@Z");
    LOAD(p_copy, "?_Internal_copy@_Concurrent_vector_base_v4@details@Concurrency@@IEAAXAEBV123@_KP6AXPEAXPEBX1@Z@Z");
    LOAD(p_assign, "?_Internal_assign@_Concurrent_vector_base_v4@details@Concurrency@@IEAAXAEBV123@_KP6AXPEAX1@ZP6AX2PEBX1@Z5@Z");
    LOAD(p_grow_by, "?_Internal_grow_by@_Concurrent_vector_base_v4@details@Concurrency@@IEAA_K_K0P6AXPEAXPEBX0@Z2@Z");
    LOAD(p_grow_to_at_least, "?_Internal_grow_to_at_least_with_result@_Concurrent_vector_base_v4@details@Concurrency@@IEAA_K_K0P6AXPEAXPEBX0@Z2@Z");
    LOAD(p_push_back, "?_Internal_push_back@_Concurrent_vector_base_v4@details@Concurrency@@IEAAPEAX_KAEA_K@Z");
    LOAD(p_resize, "?_Internal_resize@_Concurrent_vector_base_v4@details@Concurrency@@IEAAX_K00P6AXPEAX0@ZP6AX1PEBX0@Z3@Z");

    for (n = 0; n < ARRAYSIZE(sizes); n++)
    {
        int before = failures;
        es = sizes[n];
        test_first_block();
        test_append();
        test_assign();
        test_resize();
        printf("element size %2zu: %s\n", es, failures == before ? "ok" : "FAILED");
    }
    printf("%s: %d checks, %d failures\n", failures ? "FAILED" : "PASSED", checks, failures);
    return failures ? 1 : 0;
}
