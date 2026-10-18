/* Synthetic account-role protocol probe. No credentials or real entitlements. */
#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xuser.h"
#include "xstore.h"
#include "xpackage.h"
#include "xasyncprovider.h"

static IXUserImpl6 *users;
static IXStoreImpl6 *store;
static IXThreadingImpl *threading;
static IXPackageImpl3 *package;
static IXUserGamertagImpl *gamertags;
static unsigned int failures, checks;
#define CHECK(c,m) do { checks++; if (!(c)) { printf("FAIL: %s\n",m); failures++; } else printf("ok: %s\n",m); } while(0)

/* `extra` adds fixture knobs (see account_roles_fixture.py), e.g. "fault":"drop". */
static void control_ex(unsigned int owner,unsigned int player,unsigned int same,unsigned int licensed,unsigned int broken,const char *extra)
{
    FILE *file=fopen("account-control.tmp","w");
    if (!file) ExitProcess(10);
    fprintf(file,"{\"store\":%u,\"player\":%u,\"same\":%u,\"licensed\":%u,\"broken\":%u%s%s}",
        owner,player,same,licensed,broken,extra[0]?",":"",extra);
    fclose(file);
    if (!MoveFileExA("account-control.tmp","account-control.json",MOVEFILE_REPLACE_EXISTING)) ExitProcess(10);
}
static void control(unsigned int owner,unsigned int player,unsigned int same,unsigned int licensed,unsigned int broken)
{
    control_ex(owner,player,same,licensed,broken,"");
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


/* ---- runtime-to-service robustness: identity, tokens, cached roles ------ */

#ifndef E_GAMEUSER_NO_AUTH_USER
#define E_GAMEUSER_NO_AUTH_USER ((HRESULT)0x87DD0013L)
#endif
#define FIXTURE_XUID 2533274800000001ull

static XUserHandle g_user;
static XStoreLicenseHandle g_license;
static char g_message[256];
#define CHECKF(c,...) do { snprintf(g_message,sizeof(g_message),__VA_ARGS__); CHECK(c,g_message); } while(0)

static unsigned int fixture_count(const char *name)
{
    FILE *file=fopen(name,"r");
    unsigned int value=0;
    if(file) { if(fscanf(file,"%u",&value)!=1) value=0; fclose(file); }
    return value;
}

static double fixture_seconds(const char *name)
{
    FILE *file=fopen(name,"r");
    double value=0;
    if(file) { if(fscanf(file,"%lf",&value)!=1) value=0; fclose(file); }
    return value;
}

static BOOL eventually(BOOL (*probe)(void),DWORD ms)
{
    ULONGLONG until=GetTickCount64()+ms;
    while(GetTickCount64()<until) { if(probe()) return TRUE; Sleep(50); }
    return probe();
}
static BOOL probe_store_user(void) { return IXUserImpl6_XUserIsStoreUser(users,g_user); }
static BOOL probe_not_store_user(void) { return !IXUserImpl6_XUserIsStoreUser(users,g_user); }
static BOOL probe_signed_out(void)
{
    XUserState state=XUserState_SignedIn;
    return IXUserImpl6_XUserGetState(users,g_user,&state)==S_OK && state==XUserState_SignedOut;
}
static BOOL probe_licence_valid(void) { return IXStoreImpl6_XStoreIsLicenseValid(store,g_license); }
static BOOL probe_licence_invalid(void) { return !IXStoreImpl6_XStoreIsLicenseValid(store,g_license); }
static BOOL probe_live_identity(void)
{
    UINT64 id=0;
    return IXUserImpl6_XUserGetId(users,g_user,&id)==S_OK && id==FIXTURE_XUID;
}

/* Bytes the process heaps have handed out and not taken back. */
static SIZE_T heap_in_use(void)
{
    HANDLE heaps[64];
    PROCESS_HEAP_ENTRY entry;
    DWORD count=GetProcessHeaps(64,heaps),i;
    SIZE_T total=0;
    for(i=0;i<count && i<64;i++)
    {
        if(!HeapLock(heaps[i])) continue;
        entry.lpData=NULL;
        while(HeapWalk(heaps[i],&entry)) if(entry.wFlags & PROCESS_HEAP_ENTRY_BUSY) total+=entry.cbData;
        HeapUnlock(heaps[i]);
    }
    return total;
}

static XStoreLicenseHandle acquire_licence(void)
{
    XStoreLicenseHandle licence=NULL;
    XAsyncBlock async={0};
    char identifier[33];
    if(IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier(package,sizeof(identifier),identifier)!=S_OK ||
        IXStoreImpl6_XStoreAcquireLicenseForPackageAsync(store,NULL,identifier,&async)!=S_OK || wait_for(&async)!=S_OK ||
        IXStoreImpl6_XStoreAcquireLicenseForPackageResult(store,&async,&licence)!=S_OK) return NULL;
    return licence;
}

/* Runs first: the identity is resolved once per process. With Xbox Live
 * unreachable, XUserGetId used to make the request itself, on the caller's
 * thread, on every call. */
static void test_identity_off_the_callers_thread(void)
{
    XAsyncBlock async={0};
    ULONGLONG start,one,slowest=0;
    char gamertag[64]={0};
    unsigned int i,requests;
    UINT64 id=1;

    control_ex(1,1,0,1,0,"\"xsts\":\"drop\"");
    CHECK(IXUserImpl6_XUserAddAsync(users,XUserAddOptions_AddDefaultUserSilently,&async)==S_OK &&
        IXUserImpl6_XUserAddResult(users,&async,&g_user)==S_OK && g_user,"player user is available while Xbox Live is unreachable");
    if(!g_user) return;
    start=GetTickCount64();
    for(i=0;i<10;i++)
    {
        one=GetTickCount64();
        IXUserImpl6_XUserGetId(users,g_user,&id);
        one=GetTickCount64()-one;
        if(i && one>slowest) slowest=one;
    }
    CHECKF(GetTickCount64()-start<2000,"ten identity reads with Xbox Live unreachable take %llu ms",GetTickCount64()-start);
    CHECKF(slowest<100,"identity reads after the first do not wait on the service (slowest %llu ms)",slowest);
    requests=fixture_count("xsts-requests");
    CHECKF(requests<=4,"a failed identity lookup is not repeated on every read (%u requests)",requests);
    CHECK(id!=FIXTURE_XUID,"no identity is reported before one was issued");
    control(1,1,0,1,0);
    CHECK(eventually(probe_live_identity,20000),"the identity is picked up once Xbox Live answers again");
    CHECK(IXUserImpl6_QueryInterface(users,&IID_IXUserGamertagImpl,(void **)&gamertags)==S_OK &&
        IXUserGamertagImpl_XUserGetGamertag(gamertags,g_user,XUserGamertagComponent_Classic,sizeof(gamertag),gamertag,NULL)==S_OK &&
        !strcmp(gamertag,"Fixture"),"the gamertag comes with it");
}

static HRESULT token_request(UINT32 options,XAsyncBlock *async,BYTE *buffer,SIZE_T size,XUserGetTokenAndSignatureData **data)
{
    HRESULT hr;
    memset(async,0,sizeof(*async));
    *data=NULL;
    hr=IXUserImpl6_XUserGetTokenAndSignatureAsync(users,g_user,options,"GET","https://fixture.invalid/",0,NULL,0,NULL,async);
    if(FAILED(hr)) return hr;
    if((hr=wait_for(async))!=S_OK) return hr;
    return IXUserImpl6_XUserGetTokenAndSignatureResult(users,async,size,buffer,data,NULL);
}

static void test_token_requests(void)
{
    XUserGetTokenAndSignatureData *data=NULL;
    BYTE *buffer=malloc(8192);
    XAsyncBlock async={0};
    ULONGLONG start;
    SIZE_T before,after;
    unsigned int i,failed=0;
    HRESULT hr;

    control_ex(1,1,0,1,0,"\"xsts\":\"stall\"");
    start=GetTickCount64();
    hr=IXUserImpl6_XUserGetTokenAndSignatureAsync(users,g_user,0,"GET","https://fixture.invalid/",0,NULL,0,NULL,&async);
    CHECKF(hr==S_OK && GetTickCount64()-start<250,"a token request returns before the service answers (%llu ms)",GetTickCount64()-start);
    if(hr==S_OK) CHECK(wait_for(&async)==E_GAMEUSER_NO_AUTH_USER,"an unanswered token request completes as not signed in");
    control(1,1,0,1,0);

    hr=token_request(XUserGetTokenAndSignatureOptions_ForceRefresh,&async,buffer,8192,&data);
    CHECK(hr==S_OK && data && strstr(data->token,";forced;"),"ForceRefresh reaches the service");
    hr=token_request(0,&async,buffer,8192,&data);
    CHECK(hr==S_OK && data && strstr(data->token,";cached;"),"an ordinary request does not force a refresh");
    CHECK(hr==S_OK && data && data->signature && !strcmp(data->signature,"fixture-signature"),"the signature comes with the token");
    data=NULL;
    hr=IXUserImpl6_XUserGetTokenAndSignatureResult(users,&async,8192,buffer,&data,NULL);
    CHECK(hr!=S_OK,"a token result is handed over once and then released");

    token_request(0,&async,buffer,8192,&data);
    before=heap_in_use();
    for(i=0;i<1500;i++) if(token_request(0,&async,buffer,8192,&data)!=S_OK) failed++;
    after=heap_in_use();
    CHECKF(!failed,"1500 token requests succeed (%u failed)",failed);
    CHECKF(after<before+1024*1024,"1500 token requests do not leave their payloads behind (%ld KB)",
        (long)(((LONG_PTR)after-(LONG_PTR)before)/1024));
    free(buffer);
}

/* Role state used to be polled on the title's own queue and asked for on the
 * title's own thread; a stalled service then held up the title. */
static volatile LONG probe_ran;
static void CALLBACK probe_callback(void *context,BOOLEAN cancelled) { InterlockedExchange(&probe_ran,1); }

static void test_roles_off_the_title_queue(void)
{
    XTaskQueueRegistrationToken user_token={0},game_token={0},package_token={0};
    XStoreContextHandle context=NULL;
    XTaskQueueHandle queue=NULL;
    ULONGLONG start,took;
    XUserState state=XUserState_SignedOut;
    BOOLEAN answer;

    control(1,1,1,1,0);
    CHECK(eventually(probe_store_user,10000),"the runtime knows the Store user before the stall test");
    CHECK(IXStoreImpl6_XStoreCreateContext(store,g_user,&context)==S_OK,"Store context for the stall test");
    CHECK((g_license=acquire_licence())!=NULL,"package licence for the stall test");
    CHECK(IXThreadingImpl_XTaskQueueCreate(threading,XTaskQueueDispatchMode_Manual,XTaskQueueDispatchMode_Manual,&queue)==S_OK,"manual title queue created");
    if(!context || !g_license || !queue) return;
    IXUserImpl6_XUserRegisterForChangeEvent(users,queue,NULL,user_changed,&user_token);
    IXStoreImpl6_XStoreRegisterGameLicenseChanged(store,context,queue,NULL,game_changed,&game_token);
    IXStoreImpl6_XStoreRegisterPackageLicenseLost(store,g_license,queue,NULL,package_lost,&package_token);

    control_ex(1,1,1,1,0,"\"fault\":\"stall\"");
    Sleep(1500);
    InterlockedExchange(&probe_ran,0);
    start=GetTickCount64();
    CHECK(IXThreadingImpl_XTaskQueueSubmitCallback(threading,queue,XTaskQueuePort_Work,NULL,probe_callback)==S_OK,"title work submitted");
    while(!probe_ran && GetTickCount64()-start<10000)
    {
        IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Work,0);
        IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,0);
        Sleep(5);
    }
    took=GetTickCount64()-start;
    CHECKF(probe_ran && took<500,"title work is not queued behind a stalled role poll (%llu ms)",took);

    start=GetTickCount64();
    answer=IXStoreImpl6_XStoreIsLicenseValid(store,g_license);
    took=GetTickCount64()-start;
    CHECKF(answer && took<100,"XStoreIsLicenseValid answers from memory while the service stalls (%llu ms)",took);
    start=GetTickCount64();
    CHECK(IXUserImpl6_XUserGetState(users,g_user,&state)==S_OK && state==XUserState_SignedIn,"XUserGetState keeps the player signed in");
    took=GetTickCount64()-start;
    CHECKF(took<100,"XUserGetState answers from memory while the service stalls (%llu ms)",took);
    start=GetTickCount64();
    answer=IXUserImpl6_XUserIsStoreUser(users,g_user);
    took=GetTickCount64()-start;
    CHECKF(answer && took<100,"XUserIsStoreUser answers from memory while the service stalls (%llu ms)",took);

    control(1,1,1,1,0);
    IXUserImpl6_XUserUnregisterForChangeEvent(users,user_token,TRUE);
    IXStoreImpl6_XStoreUnregisterGameLicenseChanged(store,context,game_token,TRUE);
    IXStoreImpl6_XStoreUnregisterPackageLicenseLost(store,g_license,package_token,TRUE);
    while(IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,0)) {}
    IXThreadingImpl_XTaskQueueCloseHandle(threading,queue);
    IXStoreImpl6_XStoreCloseLicenseHandle(store,g_license);
    g_license=NULL;
    IXStoreImpl6_XStoreCloseContextHandle(store,context);
}

static void quiet_period(XTaskQueueHandle queue,DWORD ms)
{
    ULONGLONG until=GetTickCount64()+ms;
    while(GetTickCount64()<until)
    {
        IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,10);
        Sleep(10);
    }
}

/* A failed exchange is not an answer: it used to sign the player out, report
 * the licence lost, and invalidate the licence handle for good. */
static void test_transport_errors_keep_state(void)
{
    static const char *faults[] = { "\"fault\":\"drop\"", "\"down\":1" };
    static const char *names[] = { "a dropped connection", "a missing service" };
    XTaskQueueRegistrationToken user_token={0},game_token={0},package_token={0};
    XStoreContextHandle context=NULL;
    XTaskQueueHandle queue=NULL;
    LONG users_before,games_before,packages_before;
    XUserState state;
    unsigned int i;

    control(1,1,1,1,0);
    CHECK(eventually(probe_store_user,10000),"the runtime knows the Store user before the outage test");
    CHECK(IXStoreImpl6_XStoreCreateContext(store,g_user,&context)==S_OK,"Store context for the outage test");
    CHECK((g_license=acquire_licence())!=NULL,"package licence for the outage test");
    CHECK(IXThreadingImpl_XTaskQueueCreate(threading,XTaskQueueDispatchMode_ThreadPool,XTaskQueueDispatchMode_Manual,&queue)==S_OK,"outage notification queue created");
    if(!context || !g_license || !queue) return;
    IXUserImpl6_XUserRegisterForChangeEvent(users,queue,NULL,user_changed,&user_token);
    IXStoreImpl6_XStoreRegisterGameLicenseChanged(store,context,queue,NULL,game_changed,&game_token);
    IXStoreImpl6_XStoreRegisterPackageLicenseLost(store,g_license,queue,NULL,package_lost,&package_token);
    quiet_period(queue,1500);
    users_before=user_events; games_before=game_events; packages_before=package_events;

    for(i=0;i<2;i++)
    {
        control_ex(1,1,1,1,0,faults[i]);
        quiet_period(queue,3000);
        CHECKF(IXStoreImpl6_XStoreIsLicenseValid(store,g_license),"%s does not invalidate the licence",names[i]);
        state=XUserState_SignedOut;
        CHECKF(IXUserImpl6_XUserGetState(users,g_user,&state)==S_OK && state==XUserState_SignedIn,
            "%s does not sign the player out",names[i]);
        CHECKF(IXUserImpl6_XUserIsStoreUser(users,g_user),"%s keeps the Store user",names[i]);
        CHECKF(user_events==users_before && game_events==games_before && package_events==packages_before,
            "%s raises no sign-out or licence notifications",names[i]);
        control(1,1,1,1,0);
        quiet_period(queue,1500);
    }
    control(0,1,1,1,0);
    CHECK(eventually(probe_licence_invalid,5000),"a definitive Store sign-out still invalidates the licence");
    control(1,1,1,1,0);
    CHECK(eventually(probe_licence_valid,5000),"the licence recovers when the Store account signs back in");

    IXUserImpl6_XUserUnregisterForChangeEvent(users,user_token,TRUE);
    IXStoreImpl6_XStoreUnregisterGameLicenseChanged(store,context,game_token,TRUE);
    IXStoreImpl6_XStoreUnregisterPackageLicenseLost(store,g_license,package_token,TRUE);
    while(IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,0)) {}
    IXThreadingImpl_XTaskQueueCloseHandle(threading,queue);
    IXStoreImpl6_XStoreCloseLicenseHandle(store,g_license);
    g_license=NULL;
    IXStoreImpl6_XStoreCloseContextHandle(store,context);
}

/* A child started while a request is open must not inherit the connection:
 * the service would keep it open until the child exits. XUserAddAsync asks
 * the service on the calling thread (bounded to 5 s), so a second thread
 * calling it holds a connection open while this one starts the child. */
static DWORD WINAPI add_user_thread(void *arg)
{
    XAsyncBlock async={0};
    IXUserImpl6_XUserAddAsync(users,XUserAddOptions_AddDefaultUserSilently,&async);
    return 0;
}

static void test_sockets_close_on_exec(void)
{
    STARTUPINFOA startup={sizeof(startup)};
    PROCESS_INFORMATION process={0};
    char path[MAX_PATH],command[MAX_PATH+32];
    HANDLE thread;
    double held;

    control_ex(1,1,1,1,0,"\"fault\":\"hold\"");
    thread=CreateThread(NULL,0,add_user_thread,NULL,0,NULL);
    Sleep(1000);
    GetModuleFileNameA(NULL,path,sizeof(path));
    snprintf(command,sizeof(command),"\"%s\" --sleep 12",path);
    CHECK(CreateProcessA(path,command,NULL,NULL,FALSE,0,NULL,NULL,&startup,&process),"child process started during a request");
    if(process.hProcess)
    {
        WaitForSingleObject(process.hProcess,30000);
        CloseHandle(process.hProcess);
        CloseHandle(process.hThread);
    }
    if(thread) { WaitForSingleObject(thread,30000); CloseHandle(thread); }
    control(1,1,1,1,0);
    Sleep(1000);
    held=fixture_seconds("hold-max");
    CHECKF(held>0 && held<9,"the service connection closes when the runtime gives up, not when a child exits (%.1f s)",held);
}

/* Run under WINEDEBUG=+gdkc by the harness, which then checks the log. */
static int log_probe(void)
{
    XUserGetTokenAndSignatureData *data=NULL;
    BYTE *buffer=malloc(8192);
    XAsyncBlock async={0};
    if(IXUserImpl6_XUserAddAsync(users,XUserAddOptions_AddDefaultUserSilently,&async)!=S_OK ||
        IXUserImpl6_XUserAddResult(users,&async,&g_user)!=S_OK) return 21;
    memset(&async,0,sizeof(async));
    if(IXStoreImpl6_XStoreGetUserCollectionsIdAsync(store,NULL,"SYNTHETIC_TICKET_DO_NOT_LOG","SYNTHETIC_PUBLISHER_DO_NOT_LOG",&async)!=S_OK) return 22;
    memset(&async,0,sizeof(async));
    if(IXStoreImpl6_XStoreGetUserPurchaseIdAsync(store,NULL,"SYNTHETIC_TICKET_DO_NOT_LOG","SYNTHETIC_PUBLISHER_DO_NOT_LOG",&async)!=S_OK) return 23;
    if(token_request(0,&async,buffer,8192,&data)!=S_OK || !data) return 24;
    free(buffer);
    return 0;
}

int main(int argc,char **argv)
{
    HMODULE runtime=LoadLibraryA("xgameruntime.dll");
    HRESULT (WINAPI *query)(REFCLSID,REFIID,void **);
    XUserHandle user=NULL;
    XStoreContextHandle context=NULL;
    XStoreLicenseHandle license=NULL;
    XStoreProductQueryHandle products=NULL;
    XStoreCanAcquireLicenseResult can={0};
    XAsyncBlock async={0};
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
    if(argc>2 && !strcmp(argv[1],"--sleep")) { Sleep(atoi(argv[2])*1000); return 0; }
    if(argc>1 && !strcmp(argv[1],"--log-probe")) return log_probe();
    SetEnvironmentVariableA("XGR_ACCOUNT_ROLES_V1","1");
    test_identity_off_the_callers_thread();
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
    g_license=license;
    if(license) CHECK(eventually(probe_licence_invalid,5000),"Store sign-out invalidates the retained package licence");
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
    g_user=user;
    control(1,1,1,1,0);
    CHECK(eventually(probe_store_user,5000),"same verified MSA is the Store user");
    control(0,1,1,1,0);
    CHECK(eventually(probe_not_store_user,5000),"no Store sign-in means false even with a matching identity flag");
    control(1,0,1,1,0);
    CHECK(eventually(probe_not_store_user,5000),"signed-out player is not a Store user");
    CHECK(eventually(probe_signed_out,5000),"player sign-out is reflected in XUser state");
    CHECK(event_wait(&user_events,1,queue) && last_user_event==XUserChangeEvent_SignedOut,"Player sign-out delivers XUser SignedOut");
    CHECK(IXUserImpl6_XUserUnregisterForChangeEvent(users,user_token,TRUE),"player callback unregisters");
    control(1,1,1,1,0);
    Sleep(1500);
    while(IXThreadingImpl_XTaskQueueDispatch(threading,queue,XTaskQueuePort_Completion,0)) {}
    CHECK(user_events==1 && game_events==1 && package_events==1,"unregistered callbacks do not fire after sign-in");
    IXThreadingImpl_XTaskQueueCloseHandle(threading,queue);
    IXStoreImpl6_XStoreCloseContextHandle(store,context);
    control(1,1,1,1,0);
    CHECK(eventually(probe_store_user,5000),"the Store user is back before the robustness checks");
    test_token_requests();
    test_roles_off_the_title_queue();
    test_transport_errors_keep_state();
    test_sockets_close_on_exec();
    control(1,1,1,1,1);
    CHECK(eventually(probe_not_store_user,5000),"unsupported role responses fail closed");
    printf("%s (%u checks, %u failures)\n",failures?"FAILED":"PASSED",checks,failures);
    return failures?1:0;
}
