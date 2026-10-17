/*
 * D3D12 regression for an allocator released while a list records from it.
 *
 * Retro Classics' Ultralight renderer releases its last reference to a command
 * allocator while a command list recorded from it is still open, then closes
 * the list. Stock vkd3d-proton freed the recording VkCommandBuffer with the
 * allocator and faulted in vkEndCommandBuffer. patches/vkd3d-proton/0001 makes
 * that safe in two ways, and this drives each one:
 *
 *   retain      with VKD3D_CONFIG=retain_recording_allocators the list keeps
 *               the allocator alive: Close succeeds, the list executes and the
 *               GPU finishes it, and a Reset onto a new allocator works.
 *   invalidate  without the setting the list is invalidated: Close fails
 *               instead of faulting, and a Reset onto a new allocator works.
 *
 * It needs a Vulkan-capable GPU, so no script runs it; tests/run-tests.sh only
 * compiles it so it cannot rot. No window or game data is involved.
 *
 * Build (Proton SDK container, d3d12.h from mingw-w64; d3d12.dll is loaded at
 * run time, so no import library is needed):
 *   x86_64-w64-mingw32-gcc -O1 -o d3d12_allocator_lifetime.exe tests/d3d12_allocator_lifetime.c
 *
 * Run under the runtime being tested:
 *   VKD3D_CONFIG=retain_recording_allocators wine d3d12_allocator_lifetime.exe retain [report.txt]
 *   VKD3D_CONFIG= wine d3d12_allocator_lifetime.exe invalidate [report.txt]
 *
 * Exit status: 0 all checks passed, 1 a check failed, 2 bad usage or no
 * device, 10 the process faulted.
 */

#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <d3d12.h>
#include <stdio.h>
#include <string.h>

static unsigned int checks, failures;

#define CHECK(cond, ...) do { \
    checks++; \
    if (cond) printf( "ok   %u - ", checks ); \
    else { failures++; printf( "FAIL %u - ", checks ); } \
    printf( __VA_ARGS__ ); printf( "\n" ); fflush( stdout ); \
} while (0)

/* The fault being regressed is an access violation inside vkd3d-proton; report
 * it as a result rather than leaving Wine to print a backtrace. */
static LONG WINAPI report_fault( EXCEPTION_POINTERS *info )
{
    printf( "FAIL - exception 0x%08lx\n", info->ExceptionRecord->ExceptionCode );
    fflush( stdout );
    ExitProcess( 10 );
    return EXCEPTION_EXECUTE_HANDLER;
}

/* Submitting is what a retained allocator is for: the recording it kept alive
 * has to reach the GPU and complete, not merely close. */
static void execute_and_wait( ID3D12Device *device, ID3D12GraphicsCommandList *list )
{
    D3D12_COMMAND_QUEUE_DESC desc = { D3D12_COMMAND_LIST_TYPE_DIRECT };
    ID3D12CommandQueue *queue = NULL;
    ID3D12CommandList *lists[1];
    ID3D12Fence *fence = NULL;
    HANDLE event;
    HRESULT hr;

    hr = ID3D12Device_CreateCommandQueue( device, &desc, &IID_ID3D12CommandQueue, (void **)&queue );
    CHECK( SUCCEEDED(hr) && queue, "create a direct queue (0x%08lx)", hr );
    hr = ID3D12Device_CreateFence( device, 0, D3D12_FENCE_FLAG_NONE, &IID_ID3D12Fence, (void **)&fence );
    CHECK( SUCCEEDED(hr) && fence, "create a fence (0x%08lx)", hr );
    event = CreateEventA( NULL, FALSE, FALSE, NULL );
    if (queue && fence && event)
    {
        lists[0] = (ID3D12CommandList *)list;
        ID3D12CommandQueue_ExecuteCommandLists( queue, 1, lists );
        hr = ID3D12CommandQueue_Signal( queue, fence, 1 );
        CHECK( SUCCEEDED(hr), "signal after executing the retained list (0x%08lx)", hr );
        hr = ID3D12Fence_SetEventOnCompletion( fence, 1, event );
        CHECK( SUCCEEDED(hr) && WaitForSingleObject( event, 10000 ) == WAIT_OBJECT_0,
               "the GPU finishes the retained list" );
        hr = ID3D12Device_GetDeviceRemovedReason( device );
        CHECK( hr == S_OK, "the device is not removed (0x%08lx)", hr );
    }
    if (event) CloseHandle( event );
    if (fence) ID3D12Fence_Release( fence );
    if (queue) ID3D12CommandQueue_Release( queue );
}

int main( int argc, char **argv )
{
    HRESULT (WINAPI *create_device)( IUnknown *, D3D_FEATURE_LEVEL, REFIID, void ** );
    ID3D12CommandAllocator *allocator = NULL, *replacement = NULL;
    ID3D12GraphicsCommandList *list = NULL;
    ID3D12Device *device = NULL;
    HMODULE d3d12;
    BOOL retain;
    HRESULT hr;

    if (argc < 2 || (strcmp( argv[1], "retain" ) && strcmp( argv[1], "invalidate" )))
    {
        fprintf( stderr, "usage: %s retain|invalidate [report]\n", argv[0] );
        return 2;
    }
    retain = !strcmp( argv[1], "retain" );
    if (argc > 2 && !freopen( argv[2], "w", stdout )) return 2;
    SetUnhandledExceptionFilter( report_fault );

    if (!(d3d12 = LoadLibraryA( "d3d12.dll" ))) return 2;
    if (!(create_device = (void *)GetProcAddress( d3d12, "D3D12CreateDevice" ))) return 2;
    hr = create_device( NULL, D3D_FEATURE_LEVEL_11_0, &IID_ID3D12Device, (void **)&device );
    CHECK( SUCCEEDED(hr) && device, "create a D3D12 device (0x%08lx)", hr );
    if (!device) return 2;

    hr = ID3D12Device_CreateCommandAllocator( device, D3D12_COMMAND_LIST_TYPE_DIRECT,
            &IID_ID3D12CommandAllocator, (void **)&allocator );
    CHECK( SUCCEEDED(hr) && allocator, "create an allocator (0x%08lx)", hr );
    if (!allocator) return 2;
    /* A new list starts recording, so the allocator is now in use by it. */
    hr = ID3D12Device_CreateCommandList( device, 0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator, NULL,
            &IID_ID3D12GraphicsCommandList, (void **)&list );
    CHECK( SUCCEEDED(hr) && list, "create a recording command list (0x%08lx)", hr );
    if (!list) return 2;

    /* A precondition rather than a check of the fix: unless this is the last
     * public reference, nothing below exercises a released allocator. */
    CHECK( ID3D12CommandAllocator_Release( allocator ) == 0,
           "the test held the allocator's last public reference" );

    hr = ID3D12GraphicsCommandList_Close( list );
    if (retain)
    {
        CHECK( hr == S_OK, "the list closes on its retained allocator (0x%08lx)", hr );
        if (hr == S_OK) execute_and_wait( device, list );
    }
    else
    {
        /* Not executed: a list that failed to close is invalid to submit. */
        CHECK( FAILED(hr), "the invalidated list fails to close instead of faulting (0x%08lx)", hr );
    }

    hr = ID3D12Device_CreateCommandAllocator( device, D3D12_COMMAND_LIST_TYPE_DIRECT,
            &IID_ID3D12CommandAllocator, (void **)&replacement );
    CHECK( SUCCEEDED(hr) && replacement, "create a replacement allocator (0x%08lx)", hr );
    if (replacement)
    {
        hr = ID3D12GraphicsCommandList_Reset( list, replacement, NULL );
        CHECK( hr == S_OK, "reset onto the replacement allocator (0x%08lx)", hr );
        if (hr == S_OK)
        {
            hr = ID3D12GraphicsCommandList_Close( list );
            CHECK( hr == S_OK, "the reset list closes (0x%08lx)", hr );
        }
    }

    ID3D12GraphicsCommandList_Release( list );
    if (replacement) ID3D12CommandAllocator_Release( replacement );
    CHECK( ID3D12Device_Release( device ) == 0, "no object kept a device reference" );

    printf( "%s: %u checks, %u failed\n", failures ? "FAILED" : "PASSED", checks, failures );
    return failures ? 1 : 0;
}
