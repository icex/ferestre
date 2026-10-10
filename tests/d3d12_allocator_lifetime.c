/* Optional real-GPU regression for allocator retirement.
 * Run under the tested Proton: d3d12_allocator_lifetime.exe retain <report>
 * with VKD3D_CONFIG=retain_recording_allocators. Without the flag, pass
 * invalidation and expect Close to fail safely. No window or game data.
 */
#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <d3d12.h>
#include <stdio.h>
#include <string.h>

static unsigned checks, failures;
#define CHECK(c, text) do { ++checks; if (!(c)) { ++failures; printf("FAIL: %s\n", text); } else printf("ok: %s\n", text); fflush(stdout); } while(0)
static LONG WINAPI crash(EXCEPTION_POINTERS *p) { printf("probe exception %#lx\n",p->ExceptionRecord->ExceptionCode); fflush(stdout); ExitProcess(10); return EXCEPTION_EXECUTE_HANDLER; }
int main(int argc,char **argv)
{
    ID3D12Device *device=NULL;
    ID3D12CommandAllocator *allocator=NULL, *replacement=NULL;
    ID3D12GraphicsCommandList *list=NULL;
    HRESULT (WINAPI *create_device)(IUnknown*,D3D_FEATURE_LEVEL,REFIID,void**);
    HMODULE lib;
    HRESULT hr;
    int retain=argc>1 && !strcmp(argv[1], "retain");
    if(argc>2 && !freopen(argv[2], "w", stdout)) return 2;
    SetUnhandledExceptionFilter(crash);
    lib=LoadLibraryA("d3d12.dll");
    create_device=lib?(void*)GetProcAddress(lib,"D3D12CreateDevice"):NULL;
    if(!create_device)return 2;
    hr=create_device(NULL,D3D_FEATURE_LEVEL_11_0,&IID_ID3D12Device,(void**)&device);
    CHECK(SUCCEEDED(hr)&&device,"create D3D12 device");if(!device)return 2;
    hr=ID3D12Device_CreateCommandAllocator(device,D3D12_COMMAND_LIST_TYPE_DIRECT,&IID_ID3D12CommandAllocator,(void**)&allocator);
    CHECK(SUCCEEDED(hr)&&allocator,"create allocator");if(!allocator)return 2;
    hr=ID3D12Device_CreateCommandList(device,0,D3D12_COMMAND_LIST_TYPE_DIRECT,allocator,NULL,&IID_ID3D12GraphicsCommandList,(void**)&list);
    CHECK(SUCCEEDED(hr)&&list,"create recording list");if(!list)return 2;
    CHECK(ID3D12CommandAllocator_Release(allocator)==0,"retirement keeps public COM count at zero");
    hr=ID3D12GraphicsCommandList_Close(list);
    CHECK(retain?SUCCEEDED(hr):FAILED(hr),retain?"retained recording buffer closes successfully":"invalidated list closes with an error");
    hr=ID3D12Device_CreateCommandAllocator(device,D3D12_COMMAND_LIST_TYPE_DIRECT,&IID_ID3D12CommandAllocator,(void**)&replacement);
    CHECK(SUCCEEDED(hr)&&replacement,"create replacement allocator");
    if(replacement){
      hr=ID3D12GraphicsCommandList_Reset(list,replacement,NULL);
      CHECK(SUCCEEDED(hr),"reset with replacement allocator");
      if(SUCCEEDED(hr))CHECK(SUCCEEDED(ID3D12GraphicsCommandList_Close(list)),"replacement list closes successfully");
    }
    ID3D12GraphicsCommandList_Release(list);
    if(replacement)ID3D12CommandAllocator_Release(replacement);
    CHECK(ID3D12Device_Release(device)==0,"all device references released");
    printf("%u checks, %u failed\n",checks,failures);return failures?1:0;
}
