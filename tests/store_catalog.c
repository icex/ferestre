/* Account-free native ABI/IPC fixture, or --live against Microsoft's catalog.
 * Never launches a game, purchases content or opens a save provider. */
#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <stdio.h>
#include <string.h>
#include "xstore.h"
#include "xasyncprovider.h"
static IXStoreImpl6 *store;
static IXThreadingImpl *threading;
static int failures, checks;
#define CHECK(c,m) do {checks++; if (!(c)) {printf("FAIL: %s\n",m);failures++;}} while(0)
struct enumeration {unsigned int count; BOOL stop; const XStoreProduct *retained; char ids[2048][17];};
static BOOLEAN CALLBACK enumerate(const XStoreProduct *p,void *context)
{
    struct enumeration *e=context;
    unsigned int i;
    CHECK(p && p->storeId && strlen(p->storeId)==12,"real product ID");
    CHECK(p->title && p->title[0] && p->inAppOfferToken,"owned UTF-8 product strings");
    CHECK(p->skusCount && p->skus && p->skus[0].availabilitiesCount,"SKU and availability data");
    CHECK(p->price.currencyCode && p->price.currencyCode[0],"catalog currency");
    CHECK(!p->isInUserCollection && !p->skus[0].isInUserCollection,"catalog never invents DLC ownership");
    for(i=0;i<e->count;i++) CHECK(strcmp(e->ids[i],p->storeId),"no duplicated product across pages");
    if(e->count<2048) snprintf(e->ids[e->count],17,"%s",p->storeId);
    e->count++; e->retained=p;
    return !e->stop;
}
static HRESULT wait_for(XAsyncBlock *block)
{
    unsigned int i;HRESULT hr;
    for(i=0;i<60000;i++) {
        hr=IXThreadingImpl_XAsyncGetStatus(threading,block,FALSE);
        if(hr!=E_PENDING) return hr;
        Sleep(1);
    }
    return E_PENDING;
}
int main(int argc,char **argv)
{
    HMODULE runtime=LoadLibraryA("xgameruntime.dll");
    HRESULT (WINAPI *query)(REFCLSID,REFIID,void **);
    XStoreContextHandle ctx=NULL;
    XStoreProductQueryHandle q=NULL,next=NULL,first=NULL;
    XAsyncBlock block={0};
    struct enumeration e={0}, stopped={0};
    const char *ids[]={"9N1J3D5WNSKP"};
    char copied[32];
    BOOL live=argc>1 && !strcmp(argv[1],"--live");
    HRESULT hr; ULONGLONG started;
    if(!runtime) return 10;
    query=(void *)GetProcAddress(runtime,"QueryApiImpl");
    if(!query || FAILED(query(&CLSID_XStoreImpl,&IID_IXStoreImpl6,(void **)&store)) ||
        FAILED(query(&CLSID_XThreadingImpl,&IID_IXThreadingImpl,(void **)&threading))) return 11;
    SetEnvironmentVariableA("XGR_STORE_CATALOG","1");
    CHECK(SUCCEEDED(IXStoreImpl6_XStoreCreateContext(store,NULL,&ctx)),"store context created");
    started=GetTickCount64();
    hr=IXStoreImpl6_XStoreQueryProductsAsync(store,ctx,XStoreProductKind_Durable,ids,1,NULL,0,&block);
    CHECK(hr==S_OK && GetTickCount64()-started<250,"catalog returns before network work");
    if (FAILED(hr)) return 1;
    hr=wait_for(&block);CHECK(hr==S_OK,"product request completes");
    hr=IXStoreImpl6_XStoreQueryProductsResult(store,&block,&q);CHECK(hr==S_OK && q,"product handle returned");
    if(q) {
        IXStoreImpl6_XStoreEnumerateProductsQuery(store,q,&e,enumerate);
        CHECK(e.count==1 && !strcmp(e.ids[0],ids[0]),"requested product returned");
        CHECK(!IXStoreImpl6_XStoreProductsQueryHasMorePages(store,q),"explicit query has no extra page");
        IXStoreImpl6_XStoreCloseProductsQueryHandle(store,q);q=NULL;
    }
    ZeroMemory(&block,sizeof(block));ZeroMemory(&e,sizeof(e));
    hr=IXStoreImpl6_XStoreQueryAssociatedProductsAsync(store,ctx,XStoreProductKind_Durable,2,&block);
    CHECK(hr==S_OK && wait_for(&block)==S_OK,"associated products complete");
    hr=IXStoreImpl6_XStoreQueryAssociatedProductsResult(store,&block,&q);CHECK(hr==S_OK && q,"associated handle returned");
    if(q) {
        stopped.stop=TRUE;IXStoreImpl6_XStoreEnumerateProductsQuery(store,q,&stopped,enumerate);
        CHECK(stopped.count==1,"enumeration respects callback stop");
        first=q;snprintf(copied,sizeof(copied),"%s",stopped.retained->storeId);
        for(;;) {
            unsigned int before=e.count;
            IXStoreImpl6_XStoreEnumerateProductsQuery(store,q,&e,enumerate);
            CHECK(e.count-before<=2,"page size respected");
            if(!IXStoreImpl6_XStoreProductsQueryHasMorePages(store,q)) break;
            ZeroMemory(&block,sizeof(block));next=NULL;
            CHECK(IXStoreImpl6_XStoreProductsQueryNextPageAsync(store,q,&block)==S_OK,"next page starts");
            hr=IXStoreImpl6_XStoreProductsQueryNextPageResult(store,&block,&next);
            CHECK(hr==S_OK && next,"next page handle returned");
            if(q!=first) IXStoreImpl6_XStoreCloseProductsQueryHandle(store,q);
            if(!next) break;
            q=next;
        }
        CHECK(e.count>2 && (live || e.count==3),"all associated products paged");
        CHECK(!strcmp(stopped.retained->storeId,copied),"first page strings survive subsequent pages");
        IXStoreImpl6_XStoreCloseProductsQueryHandle(store,first);
        if(q!=first) {
            stopped.count=0;IXStoreImpl6_XStoreEnumerateProductsQuery(store,q,&stopped,enumerate);
            CHECK(stopped.count>0,"closing earlier page preserves later page data");
            IXStoreImpl6_XStoreCloseProductsQueryHandle(store,q);
        }
    }
    if(!live) {
        const char *bad[]={"9BADDECODE00","9FAILQUERY00","9TRUNCATED00"}; unsigned int i;
        for(i=0;i<3;i++) {
            ids[0]=bad[i];ZeroMemory(&block,sizeof(block));q=NULL;
            CHECK(IXStoreImpl6_XStoreQueryProductsAsync(store,ctx,2,ids,1,NULL,0,&block)==S_OK,"failure case starts asynchronously");
            hr=wait_for(&block);CHECK(FAILED(hr) && hr!=E_PENDING,"bad response is an error");
            hr=IXStoreImpl6_XStoreQueryProductsResult(store,&block,&q);CHECK(FAILED(hr) && !q,"failed query returns no product handle");
        }
        ids[0]="../bad";ZeroMemory(&block,sizeof(block));
        CHECK(IXStoreImpl6_XStoreQueryProductsAsync(store,ctx,2,ids,1,NULL,0,&block)==E_INVALIDARG,"invalid Store ID refused before IPC");
    }
    IXStoreImpl6_XStoreCloseContextHandle(store,ctx);
    printf("Catalog: %d checks, %d failures; associated products %u.\n",checks,failures,e.count);
    return failures?1:0;
}
