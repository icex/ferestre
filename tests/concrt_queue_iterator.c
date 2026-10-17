/* Exercise the exported C++ ABI without depending on a particular compiler's
 * inline iterator code. The same binary can run against Microsoft's runtime. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

struct page { struct page *next; SIZE_T mask; BYTE data[1]; };
struct queue { void **vtable; void *data; SIZE_T capacity, item_size; };
struct iterator { void *data; void *item; };
static int failures;
#define CHECK(c, s) do { if (!(c)) { printf("FAIL: %s\n", s); failures++; } } while (0)
static void copy_item(struct queue *q, struct page *p, SIZE_T slot, const void *item)
{ memcpy(p->data + slot * q->item_size, item, q->item_size); }
static void pop_item(struct queue *q, void *out, struct page *p, SIZE_T slot)
{ memcpy(out, p->data + slot * q->item_size, q->item_size); }
static struct page *allocate_page(struct queue *q)
{ return calloc(1, 2 * sizeof(void *) + q->capacity * q->item_size); }
static void free_page(struct queue *q, struct page *p) { free(p); }
static void *vtable[] = { copy_item, copy_item, pop_item, NULL, allocate_page, free_page };

int main(int argc, char **argv)
{
    HMODULE dll = LoadLibraryA(argc > 1 ? argv[1] : "concrt140.dll");
    struct queue *(*ctor)(struct queue *, SIZE_T);
    void (*dtor)(struct queue *), (*clear)(struct queue *), (*push)(struct queue *, void *);
    BOOLEAN (*pop)(struct queue *, void *);
    SIZE_T (*size)(struct queue *);
    struct iterator *(*begin)(struct iterator *, struct queue *);
    void (*destroy)(struct iterator *), (*advance)(struct iterator *), (*assign)(struct iterator *, const struct iterator *);
    SIZE_T sizes[] = { 8, 64, 256 }, n, i;
    BYTE value[256], result[256];
    struct queue q;
    struct iterator it, copy = { 0 }, end = { 0 };
#define LOAD(target, symbol) do { target = (void *)GetProcAddress(dll, symbol); if (!target) { printf("Missing: %s\n", symbol); return 100; } } while (0)
    if (!dll) return 100;
    LOAD(ctor, "??0_Concurrent_queue_base_v4@details@Concurrency@@IEAA@_K@Z");
    LOAD(dtor, "??1_Concurrent_queue_base_v4@details@Concurrency@@MEAA@XZ");
    LOAD(clear, "?_Internal_finish_clear@_Concurrent_queue_base_v4@details@Concurrency@@IEAAXXZ");
    LOAD(push, "?_Internal_push@_Concurrent_queue_base_v4@details@Concurrency@@IEAAXPEBX@Z");
    LOAD(pop, "?_Internal_pop_if_present@_Concurrent_queue_base_v4@details@Concurrency@@IEAA_NPEAX@Z");
    LOAD(size, "?_Internal_size@_Concurrent_queue_base_v4@details@Concurrency@@IEBA_KXZ");
    LOAD(begin, "??0_Concurrent_queue_iterator_base_v4@details@Concurrency@@IEAA@AEBV_Concurrent_queue_base_v4@12@@Z");
    LOAD(destroy, "??1_Concurrent_queue_iterator_base_v4@details@Concurrency@@IEAA@XZ");
    LOAD(advance, "?_Advance@_Concurrent_queue_iterator_base_v4@details@Concurrency@@IEAAXXZ");
    LOAD(assign, "?_Assign@_Concurrent_queue_iterator_base_v4@details@Concurrency@@IEAAXAEBV123@@Z");
    for (n = 0; n < sizeof(sizes) / sizeof(sizes[0]); n++)
    {
        ctor(&q, sizes[n]); q.vtable = vtable;
        CHECK(begin(&it, &q) == &it && it.item == NULL, "empty begin is end and constructor returns this");
        destroy(&it); CHECK(it.data == NULL, "destructor clears owned iterator state");
        for (i = 0; i < 600; i++)
        {
            memset(value, (BYTE)i, q.item_size); *(UINT64 *)value = i; push(&q, value);
        }
        begin(&it, &q);
        for (i = 0; i < 600; i++)
        {
            CHECK(it.item != NULL, "iterator visits every item across pages");
            if (!it.item) break;
            memset(value, (BYTE)i, q.item_size); *(UINT64 *)value = i;
            CHECK(!memcmp(it.item, value, q.item_size), "iterator returns full FIFO payload");
            if (i == 37)
            {
                assign(&copy, &it); assign(&copy, &copy);
                CHECK(copy.item == it.item && copy.data != it.data, "copy has independent state; self assignment preserves it");
            }
            advance(&it);
        }
        CHECK(it.item == NULL && size(&q) == 600, "iteration reaches end without consuming queue elements");
        CHECK(*(UINT64 *)copy.item == 37, "advancing original does not advance copy");
        advance(&copy); CHECK(*(UINT64 *)copy.item == 38, "copied cursor advances independently");
        assign(&copy, &end); CHECK(!copy.data && !copy.item, "assigning end frees prior state"); destroy(&copy);
        destroy(&it);
        for (i = 0; i < 257; i++) CHECK(pop(&q, result) && *(UINT64 *)result == i, "queue pop still works after iteration");
        begin(&it, &q);
        for (i = 257; i < 600; i++)
        {
            CHECK(it.item && *(UINT64 *)it.item == i, "begin after partial consumption follows remaining pages");
            if (!it.item) break;
            advance(&it);
        }
        CHECK(it.item == NULL, "partially consumed queue iterator reaches end"); destroy(&it);
        for (i = 257; i < 600; i++) CHECK(pop(&q, result) && *(UINT64 *)result == i, "iterator preserves remaining queue values");
        CHECK(!pop(&q, result), "queue is empty only after popping all values");
        clear(&q); dtor(&q);
        printf("Checked queue payload size %Iu\n", sizes[n]);
    }
    printf("%s: %d failed\n", failures ? "FAILED" : "PASSED", failures);
    return failures;
}
