/* Synthetic account-role protocol probe. No credentials or real entitlements. */
#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <stdio.h>
#include <string.h>
#include "xuser.h"
#include "xstore.h"
#include "xpackage.h"
#include "xasyncprovider.h"

static IXUserImpl6 *users;
static IXStoreImpl6 *store;
static IXThreadingImpl *threading;
static IXPackageImpl3 *package;
static unsigned int failures, checks;
#define CHECK(c,m) do { checks++; if (!(c)) { printf("FAIL: %s\n",m); failures++; } else printf("ok: %s\n",m); } while(0)

static void control(unsigned int owner,unsigned int player,unsigned int same,unsigned int licensed,unsigned int broken)
{
    FILE *file=fopen("account-control.tmp","w");
    if (!file) ExitProcess(10);
    fprintf(file,"{\"store\":%u,\"player\":%u,\"same\":%u,\"licensed\":%u,\"broken\":%u}",owner,player,same,licensed,broken);
    fclose(file);
    if (!MoveFileExA("account-control.tmp","account-control.json",MOVEFILE_REPLACE_EXISTING)) ExitProcess(10);
}

static HRESULT wait_for(XAsyncBlock *block)
{
    unsigned int i;
    HRESULT hr;
    for(i=0;i<35000;i++) {
        hr=IXThreadingImpl_XAsyncGetStatus(threading,block,FALSE);
        if(hr!=E_PENDING) return hr;
        Sleep(1);
    }
    return E_PENDING;
}

static BOOLEAN CALLBACK count_owned(const XStoreProduct *product,void *context)
{
    unsigned int *count=context;
    CHECK(product && product->isInUserCollection,"entitled product represents the Store grant");
    (*count)++;
    return TRUE;
}

static void game_license(XStoreContextHandle context,BOOL expected)
{
    XAsyncBlock async={0};
    XStoreGameLicense license={0};
    HRESULT hr;
    ULONGLONG before=GetTickCount64();
    hr=IXStoreImpl6_XStoreQueryGameLicenseAsync(store,context,&async);
    CHECK(hr==S_OK && GetTickCount64()-before<250,"game licence returns before the delayed Store check");
    if(FAILED(hr)) return;
    CHECK(wait_for(&async)==S_OK,"Store game licence completes");
    hr=IXStoreImpl6_XStoreQueryGameLicenseResult(store,&async,&license);
    CHECK(hr==S_OK && !!license.isActive==!!expected,"game licence follows the Store entitlement");
    if(expected) CHECK(license.expirationDate!=0,"profile licence is revalidated rather than reported perpetual");
}

static volatile LONG user_events, game_events, package_events;
static volatile LONG last_user_event;
static void CALLBACK user_changed(void *context,XUserLocalId user,XUserChangeEvent event)
{
    InterlockedExchange(&last_user_event,event);InterlockedIncrement(&user_events);
}
static void CALLBACK game_changed(void *context) { InterlockedIncrement(&game_events); }
static void CALLBACK package_lost(void *context) { InterlockedIncrement(&package_events); }
static BOOL event_wait(volatile LONG *value,LONG expected,XTaskQueueHandle queue)
{
    ULONGLONG until=GetTickCount64()+12000;
    while(GetTickCount64()<until && *value<expected)
    {
        IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,10);
        Sleep(10);
    }
    return *value>=expected;
}

int main(void)
{
    HMODULE runtime=LoadLibraryA("xgameruntime.dll");
    HRESULT (WINAPI *query)(REFCLSID,REFIID,void **);
    XUserHandle user=NULL;
    XStoreContextHandle context=NULL;
    XStoreLicenseHandle license=NULL;
    XStoreProductQueryHandle products=NULL;
    XStoreCanAcquireLicenseResult can={0};
    XAsyncBlock async={0};
    XUserState state;
    char identifier[33];
    unsigned int count=0;
    HRESULT hr;
    XTaskQueueHandle queue=NULL;
    XTaskQueueRegistrationToken user_token={0}, game_token={0}, package_token={0};
    if(!runtime) return 11;
    query=(void *)GetProcAddress(runtime,"QueryApiImpl");
    if(!query || FAILED(query(&CLSID_XUserImpl,&IID_IXUserImpl6,(void **)&users)) ||
        FAILED(query(&CLSID_XStoreImpl,&IID_IXStoreImpl6,(void **)&store)) ||
        FAILED(query(&CLSID_XThreadingImpl,&IID_IXThreadingImpl,(void **)&threading)) ||
        FAILED(query(&CLSID_XPackageImpl,&IID_IXPackageImpl3,(void **)&package))) return 12;
    SetEnvironmentVariableA("XGR_ACCOUNT_ROLES_V1","1");
    control(1,1,0,1,0);
    hr=IXUserImpl6_XUserAddAsync(users,XUserAddOptions_AddDefaultUserSilently,&async);
    CHECK(hr==S_OK && IXUserImpl6_XUserAddResult(users,&async,&user)==S_OK && user,"player user is available");
    CHECK(!IXUserImpl6_XUserIsStoreUser(users,user),"different verified accounts are not the same Store user");
    CHECK(!IXUserImpl6_XUserIsStoreUser(users,NULL),"invalid user is never the Store user");
    CHECK(IXStoreImpl6_XStoreCreateContext(store,user,&context)==S_OK,"Store context for the player is available");
    game_license(context,TRUE);
    ZeroMemory(&async,sizeof(async));
    CHECK(IXStoreImpl6_XStoreQueryEntitledProductsAsync(store,context,XStoreProductKind_Game,10,&async)==S_OK &&
        wait_for(&async)==S_OK,"entitled query completes through the Store account");
    CHECK(IXStoreImpl6_XStoreQueryEntitledProductsResult(store,&async,&products)==S_OK && products,"entitled handle returned");
    if(products) {
        IXStoreImpl6_XStoreEnumerateProductsQuery(store,products,&count,count_owned);
        CHECK(count==1,"Store licence grants the current game independently of player ownership");
        IXStoreImpl6_XStoreCloseProductsQueryHandle(store,products);
    }
    CHECK(IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier(package,sizeof(identifier),identifier)==S_OK,"current package identifier available");
    ZeroMemory(&async,sizeof(async));
    CHECK(IXStoreImpl6_XStoreAcquireLicenseForPackageAsync(store,NULL,identifier,&async)==S_OK && wait_for(&async)==S_OK,"current package licence acquired from Store");
    CHECK(IXStoreImpl6_XStoreAcquireLicenseForPackageResult(store,&async,&license)==S_OK && license,"package licence handle returned");
    if(license) CHECK(IXStoreImpl6_XStoreIsLicenseValid(store,license),"verified package licence is valid");
    ZeroMemory(&async,sizeof(async));
    CHECK(IXStoreImpl6_XStoreAcquireLicenseForPackageAsync(store,NULL,"unrelated-package",&async)==E_NOTIMPL,"current licence cannot grant an unrelated package");
    CHECK(IXThreadingImpl_XTaskQueueCreate(threading,XTaskQueueDispatchMode_ThreadPool,XTaskQueueDispatchMode_Manual,&queue)==S_OK,"notification queue created");
    CHECK(IXUserImpl6_XUserRegisterForChangeEvent(users,queue,NULL,user_changed,&user_token)==S_OK,"player change callback registered");
    CHECK(IXStoreImpl6_XStoreRegisterGameLicenseChanged(store,context,queue,NULL,game_changed,&game_token)==S_OK,"Store game licence callback registered");
    CHECK(IXStoreImpl6_XStoreRegisterPackageLicenseLost(store,license,queue,NULL,package_lost,&package_token)==S_OK,"package licence callback registered");
    CHECK(!user_events && !game_events && !package_events,"registration does not invoke callbacks inline");
    control(0,1,0,1,0);
    if(license) CHECK(!IXStoreImpl6_XStoreIsLicenseValid(store,license),"Store sign-out invalidates the retained package licence");
    game_license(context,FALSE);
    CHECK(event_wait(&game_events,1,queue),"Store sign-out delivers a game licence notification on Completion");
    CHECK(event_wait(&package_events,1,queue),"Store sign-out delivers a package licence lost notification");
    CHECK(!user_events,"Store sign-out does not sign out the Player");
    CHECK(IXStoreImpl6_XStoreUnregisterPackageLicenseLost(store,license,package_token,TRUE),"package callback unregisters");
    CHECK(IXStoreImpl6_XStoreUnregisterGameLicenseChanged(store,context,game_token,TRUE),"game licence callback unregisters");
    if(license) IXStoreImpl6_XStoreCloseLicenseHandle(store,license);
    control(1,1,0,0,0);
    game_license(context,FALSE);
    ZeroMemory(&async,sizeof(async));products=NULL;count=0;
    CHECK(IXStoreImpl6_XStoreQueryEntitledProductsAsync(store,context,XStoreProductKind_Game,10,&async)==S_OK &&
        wait_for(&async)==S_OK,"a denied Store entitlement completes without a fallback grant");
    CHECK(IXStoreImpl6_XStoreQueryEntitledProductsResult(store,&async,&products)==S_OK && products,"denied query returns an empty handle");
    if(products) {
        IXStoreImpl6_XStoreEnumerateProductsQuery(store,products,&count,count_owned);
        CHECK(count==0,"an installed current game does not fabricate ownership");
        IXStoreImpl6_XStoreCloseProductsQueryHandle(store,products);
    }
    ZeroMemory(&async,sizeof(async));
    CHECK(IXStoreImpl6_XStoreCanAcquireLicenseForPackageAsync(store,context,identifier,&async)==S_OK && wait_for(&async)==S_OK,"package licensing check completes");
    CHECK(IXStoreImpl6_XStoreCanAcquireLicenseForPackageResult(store,&async,&can)==S_OK &&
        can.status==XStoreCanLicenseStatus_NotLicensableToUser,"unlicensed package is reported unlicensed");
    control(1,1,1,1,0);
    CHECK(IXUserImpl6_XUserIsStoreUser(users,user),"same verified MSA is the Store user");
    control(0,1,1,1,0);
    CHECK(!IXUserImpl6_XUserIsStoreUser(users,user),"no Store sign-in means false even with a matching identity flag");
    control(1,0,1,1,0);
    CHECK(!IXUserImpl6_XUserIsStoreUser(users,user),"signed-out player is not a Store user");
    CHECK(IXUserImpl6_XUserGetState(users,user,&state)==S_OK && state==XUserState_SignedOut,"player sign-out is reflected in XUser state");
    CHECK(event_wait(&user_events,1,queue) && last_user_event==XUserChangeEvent_SignedOut,"Player sign-out delivers XUser SignedOut");
    CHECK(IXUserImpl6_XUserUnregisterForChangeEvent(users,user_token,TRUE),"player callback unregisters");
    control(1,1,1,1,0);
    Sleep(1500);
    while(IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,0)) {}
    CHECK(user_events==1 && game_events==1 && package_events==1,"unregistered callbacks do not fire after sign-in");
    IXThreadingImpl_XTaskQueueCloseHandle(threading,queue);
    IXStoreImpl6_XStoreCloseContextHandle(store,context);
    control(1,1,1,1,1);
    CHECK(!IXUserImpl6_XUserIsStoreUser(users,user),"unsupported role responses fail closed");
    printf("%s (%u checks, %u failures)\n",failures?"FAILED":"PASSED",checks,failures);
    return failures?1:0;
}
