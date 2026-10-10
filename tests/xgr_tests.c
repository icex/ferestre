/*
 * Regression tests for the xgameruntime implementations in this repo.
 *
 * Self-contained: it drives the DLL through QueryApiImpl the way the GDK
 * static library does, against a synthetic MicrosoftGame.config fixture and a
 * scratch save root, and asserts on the results. No real game or save data is
 * needed, so it is deterministic and safe to run in CI.
 *
 * Covers XPackage (identity, locale, enumeration, mount path, chunks, install
 * state), XUser (the single local user), XThreading (async result size), and
 * XGameSave (create/write/read/round-trip/delete, buffer-too-small, quota,
 * invalid names) against the WGS on-disk format.
 *
 * Environment (set by run-tests.sh):
 *   XGR_WGS_ROOT         scratch directory for saves
 *   XGR_EXPECTED_FAMILY  package family name computed from the fixture
 *   cwd                  a directory containing the fixture MicrosoftGame.config
 *
 * Exit code is the number of failed checks (0 = all passed).
 *
 * Build (Proton SDK container):
 *   x86_64-w64-mingw32-gcc -o xgr_tests.exe xgr_tests.c -D__WINESRC__ -DCOBJMACROS \
 *       -I<obj>/dlls/xgameruntime -I<src>/dlls/xgameruntime
 */

#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "xpackage.h"
#include "xuser.h"
#include "xgamesave.h"
#include "xstore.h"
#include "xgameui.h"
#include "xpersistentlocalstorage.h"
#include "xsystem.h"
#include "xnetworking.h"
#include "xasyncprovider.h"

#define E_GS_INVALID_CONTAINER_NAME    ((HRESULT)0x80830001L)
#define E_GS_PROVIDED_BUFFER_TOO_SMALL ((HRESULT)0x80830007L)

static int g_run, g_failed;

#define CHECK(cond, ...) do { \
    g_run++; \
    if (cond) printf( "ok   %d - ", g_run ); \
    else { g_failed++; printf( "FAIL %d - ", g_run ); } \
    printf( __VA_ARGS__ ); printf( "\n" ); \
} while (0)

static IXPackageImpl3 *pkg;
static IXUserImpl *users;
static IXThreadingImpl *threading;
static IXGameSaveImpl3 *gs;
static IXStoreImpl6 *store;
static IXGameUiImpl4 *gameui;
static HRESULT (WINAPI *g_query)( REFCLSID, REFIID, void ** );
static XUserHandle user;

/* deterministic 32-bit hash matching the harness helpers */
static unsigned int hash_bytes( const unsigned char *p, size_t n )
{
    unsigned int h = 0;
    while (n--) h = h * 31u + *p++;
    return h;
}

/* ---- XPackage ----------------------------------------------------------- */

struct enum_ctx { int count; char identifier[256]; char title[64]; XVersion version; };

static BOOLEAN __stdcall count_package( void *ctx, const XPackageDetails *d )
{
    struct enum_ctx *c = ctx;
    c->count++;
    lstrcpynA( c->identifier, d->packageIdentifier ? d->packageIdentifier : "", sizeof(c->identifier) );
    lstrcpynA( c->title, d->titleID ? d->titleID : "", sizeof(c->title) );
    c->version = d->version;
    return TRUE;
}

static void test_xpackage(void)
{
    struct enum_ctx ctx = { 0 };
    XAsyncBlock async = { 0 };
    XPackageMountHandle mount = NULL;
    XPackageChunkAvailability avail = 99;
    XPackageInstallationMonitorHandle mon = NULL;
    XPackageInstallationProgress prog = { 0 };
    char buf[512];
    char identifier[33] = {0}, repeated[33] = {0}, tiny[2] = {'!', '!'};
    SIZE_T size = 0;
    HRESULT hr;

    printf( "# XPackage\n" );
    CHECK( IXPackageImpl3_XPackageIsPackagedProcess( pkg ), "IsPackagedProcess is TRUE" );

    hr = IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier( pkg, sizeof(identifier), identifier );
    CHECK( SUCCEEDED( hr ), "package identifier fits the GDK's 33-byte buffer (0x%08lx)", hr );
    CHECK( identifier[0] && identifier[32] == 0, "package identifier is nonempty and terminated within its maximum" );
    hr = IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier( pkg, sizeof(repeated), repeated );
    CHECK( hr == S_OK && !strcmp( identifier, repeated ), "package identifier stays stable within a process" );
    hr = IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier( pkg, 1, tiny );
    CHECK( FAILED(hr) && tiny[0] == '!' && tiny[1] == '!', "undersized package identifier buffer is rejected without overwriting" );

    hr = IXPackageImpl3_XPackageGetUserLocale( pkg, sizeof(buf), buf );
    CHECK( SUCCEEDED( hr ) && buf[0], "GetUserLocale returns a locale ('%s')", buf );

    hr = IXPackageImpl3_XPackageEnumeratePackages( pkg, XPackageKind_Game,
            XPackageEnumerationScope_ThisOnly, &ctx, count_package );
    CHECK( SUCCEEDED( hr ) && ctx.count == 1, "EnumeratePackages(Game) reports exactly 1 (%d)", ctx.count );
    CHECK( !strcmp( ctx.identifier, identifier ), "enumeration and current-package query identify the same installation" );
    CHECK( !strcmp( ctx.title, "4D5E6F70" ), "enumerated titleID == fixture ('%s')", ctx.title );
    CHECK( ctx.version.major == 2 && ctx.version.minor == 7 && ctx.version.build == 13 &&
           ctx.version.revision == 42, "enumerated version == 2.7.13.42 (%u.%u.%u.%u)",
           ctx.version.major, ctx.version.minor, ctx.version.build, ctx.version.revision );

    ctx.count = 0;
    hr = IXPackageImpl3_XPackageEnumeratePackages( pkg, XPackageKind_Content,
            XPackageEnumerationScope_ThisOnly, &ctx, count_package );
    CHECK( SUCCEEDED( hr ) && ctx.count == 0, "EnumeratePackages(Content) reports 0 (%d)", ctx.count );

    hr = IXPackageImpl3_XPackageMountWithUiAsync( pkg, identifier, &async );
    hr = IXPackageImpl3_XPackageMountWithUiResult( pkg, &async, &mount );
    CHECK( SUCCEEDED( hr ) && mount, "MountWithUi returns a handle (0x%08lx)", hr );
    if (mount)
    {
        hr = IXPackageImpl3_XPackageGetMountPathSize( pkg, mount, &size );
        CHECK( SUCCEEDED( hr ) && size > 1, "GetMountPathSize > 1 (%zu)", (size_t)size );
        hr = IXPackageImpl3_XPackageGetMountPath( pkg, mount, sizeof(buf), buf );
        CHECK( SUCCEEDED( hr ) && buf[0], "GetMountPath returns a path ('%s')", buf );
        CHECK( strlen( buf ) + 1 == size, "mount path length matches reported size" );
        /* buffer too small must fail cleanly, not overrun */
        hr = IXPackageImpl3_XPackageGetMountPath( pkg, mount, 1, buf );
        CHECK( FAILED( hr ), "GetMountPath with tiny buffer fails (0x%08lx)", hr );
        IXPackageImpl3_XPackageCloseMountHandle( pkg, mount );
    }

    hr = IXPackageImpl3_XPackageFindChunkAvailability( pkg, identifier, 0, NULL, &avail );
    CHECK( SUCCEEDED( hr ) && avail == XPackageChunkAvailability_Ready,
           "FindChunkAvailability is Ready (%d)", avail );

    hr = IXPackageImpl3_XPackageCreateInstallationMonitor( pkg, identifier, 0, NULL, 0, NULL, &mon );
    CHECK( SUCCEEDED( hr ) && mon, "CreateInstallationMonitor returns a handle" );
    if (mon)
    {
        IXPackageImpl3_XPackageGetInstallationProgress( pkg, mon, &prog );
        CHECK( prog.completed && prog.launchable && prog.installedBytes == prog.totalBytes,
               "installation progress is complete and launchable" );
        IXPackageImpl3_XPackageCloseInstallationMonitorHandle( pkg, mon );
    }
}

/* ---- XUser -------------------------------------------------------------- */

static void test_xuser(void)
{
    XAsyncBlock async = { 0 };
    SIZE_T result_size = 0, used = 0;
    XUserHandle generic_user = NULL;
    XUserState state = 99;
    XUserLocalId local = { 0 };
    BOOLEAN guest = TRUE;
    UINT64 id = 1;
    HRESULT hr;

    printf( "# XUser\n" );
    hr = IXUserImpl_XUserAddAsync( users, XUserAddOptions_AddDefaultUserSilently, &async );
    CHECK( SUCCEEDED( hr ), "XUserAddAsync succeeds (0x%08lx)", hr );
    hr = IXThreadingImpl_XAsyncGetResultSize( threading, &async, &result_size );
    CHECK( SUCCEEDED( hr ) && result_size == sizeof(XUserHandle),
           "XUserAdd advertises the handle result size (%Iu bytes)", result_size );
    hr = IXThreadingImpl_XAsyncGetResult( threading, &async, NULL, sizeof(generic_user) - 1,
                                        &generic_user, &used );
    CHECK( hr == HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER) && !generic_user,
           "a short generic user-result buffer is rejected without writing it" );
    hr = IXThreadingImpl_XAsyncGetResult( threading, &async, NULL, sizeof(generic_user),
                                        &generic_user, &used );
    CHECK( SUCCEEDED( hr ) && generic_user && used == sizeof(generic_user),
           "generic async retrieval returns the user handle bytes" );
    hr = IXUserImpl_XUserAddResult( users, &async, &user );
    CHECK( SUCCEEDED( hr ) && user, "XUserAddResult returns a user (0x%08lx)", hr );
    CHECK( user == generic_user, "typed and generic user results identify the same user" );
    if (!user) return;

    hr = IXUserImpl_XUserGetState( users, user, &state );
    CHECK( SUCCEEDED( hr ) && state == XUserState_SignedIn, "user state is SignedIn (%d)", state );
    hr = IXUserImpl_XUserGetLocalId( users, user, &local );
    CHECK( SUCCEEDED( hr ) && local.value != 0, "user has a non-zero local id" );
    hr = IXUserImpl_XUserGetIsGuest( users, user, &guest );
    CHECK( SUCCEEDED( hr ) && !guest, "user is not a guest" );
    hr = IXUserImpl_XUserGetId( users, user, &id );
    CHECK( SUCCEEDED( hr ), "XUserGetId succeeds (0x%08lx)", hr );

    {
        IXUserImpl6 *u6 = NULL;
        if (SUCCEEDED( g_query( &CLSID_XUserImpl, &IID_IXUserImpl6, (void **)&u6 ) ))
        {
            XUserAgeGroup age = 99;
            XUserPrivilegeDenyReason reason = 99;
            BOOLEAN has = FALSE;

            hr = IXUserImpl6_XUserGetAgeGroup( u6, user, &age );
            CHECK( SUCCEEDED( hr ) && age == XUserAgeGroup_Adult, "user age group is Adult (%d)", age );
            hr = IXUserImpl6_XUserCheckPrivilege( u6, user, 0, XUserPrivilege_CrossPlay, &has, &reason );
            CHECK( SUCCEEDED( hr ) && has, "user holds the cross-play privilege" );
            CHECK( IXUserImpl6_XUserIsStoreUser( u6, user ), "user is a store user" );

            /* The fixture returns ASCII token/signature strings over IPC. The
             * UTF-16 entry point must return actual wide strings to the game. */
            {
                XAsyncBlock token_async = {0};
                XUserGetTokenAndSignatureUtf16Data *wide = NULL;
                XUserGetTokenAndSignatureData *narrow = NULL;
                BYTE *buffer = malloc(512);
                SIZE_T size = 0, used = 0;
                SIZE_T expected = sizeof(*wide) + sizeof(L"fixture-auth") + sizeof(L"fixture-signature");

                hr = IXUserImpl6_XUserGetTokenAndSignatureUtf16Async( u6, user, 0, L"GET",
                        L"https://fixture.invalid/", 0, NULL, 0, NULL, &token_async );
                CHECK( hr == S_OK, "UTF-16 token request starts" );
                hr = IXUserImpl6_XUserGetTokenAndSignatureUtf16ResultSize( u6, &token_async, &size );
                CHECK( hr == S_OK && size == expected, "UTF-16 result size includes wide strings (%Iu)", size );
                memset(buffer, 0x5a, 512);
                hr = IXUserImpl6_XUserGetTokenAndSignatureUtf16Result( u6, &token_async, expected - 1,
                        buffer, &wide, &used );
                CHECK( hr == HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER) && buffer[0] == 0x5a,
                        "short UTF-16 buffer is rejected without writing" );
                hr = IXUserImpl6_XUserGetTokenAndSignatureUtf16Result( u6, &token_async, 512,
                        buffer, &wide, &used );
                CHECK( hr == S_OK && wide == (void *)buffer && used == expected,
                        "UTF-16 result reports the bytes written" );
                CHECK( hr == S_OK && wide && wide->tokenCount == sizeof(L"fixture-auth") &&
                        !memcmp(wide->token, L"fixture-auth", sizeof(L"fixture-auth")),
                        "UTF-16 token is a terminated wide string with a byte count" );
                CHECK( hr == S_OK && wide && wide->signatureCount == sizeof(L"fixture-signature") &&
                        !memcmp(wide->signature, L"fixture-signature", sizeof(L"fixture-signature")),
                        "UTF-16 signature is a terminated wide string with a byte count" );
                CHECK( buffer[expected] == 0x5a, "UTF-16 result leaves the tail untouched" );
                /* Retrieving the Unicode form must not corrupt the cached UTF-8 payload. */
                hr = IXUserImpl6_XUserGetTokenAndSignatureResult( u6, &token_async, 512,
                        buffer, &narrow, &used );
                CHECK( hr == S_OK && narrow && !strcmp(narrow->token, "fixture-auth") &&
                        !strcmp(narrow->signature, "fixture-signature"), "UTF-8 payload remains intact" );
                free(buffer);
            }
        }
        else CHECK( 0, "query IXUserImpl6" );
    }
}

/* ---- XGameSave ---------------------------------------------------------- */

static const char BLOB_NAME[] = "WorldState";
static unsigned char blob_payload[4096];
static unsigned int blob_expected_hash;

static BOOLEAN __stdcall find_container( const XGameSaveContainerInfo *info, void *ctx )
{
    if (!strcmp( info->name, "unit_test_container" ))
    {
        *(int *)ctx = info->blobCount;
    }
    return TRUE;
}

static void test_xgamesave( const char *scid )
{
    XGameSaveProviderHandle provider = NULL, generic_provider = NULL;
    SIZE_T result_size = 0, used = 0;
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle update = NULL;
    XAsyncBlock async = { 0 };
    XGameSaveBlob *blobs;
    unsigned int i, count = 1;
    int blob_count = -1;
    INT64 quota_before = 0, quota_after = 0;
    HRESULT hr;

    printf( "# XGameSave\n" );
    for (i = 0; i < sizeof(blob_payload); i++) blob_payload[i] = (unsigned char)(i * 7 + 3);
    blob_expected_hash = hash_bytes( blob_payload, sizeof(blob_payload) );

    hr = IXGameSaveImpl3_XGameSaveInitializeProviderAsync( gs, user, scid, FALSE, &async );
    hr = IXThreadingImpl_XAsyncGetResultSize( threading, &async, &result_size );
    CHECK( SUCCEEDED(hr) && result_size == sizeof(XGameSaveProviderHandle),
           "save initialization advertises a provider handle result (%Iu bytes)", result_size );
    hr = IXThreadingImpl_XAsyncGetResult( threading, &async, NULL, sizeof(generic_provider),
                                        &generic_provider, &used );
    CHECK( SUCCEEDED(hr) && generic_provider && used == sizeof(generic_provider),
           "generic async retrieval returns the save provider handle" );
    hr = IXGameSaveImpl3_XGameSaveInitializeProviderResult( gs, &async, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "InitializeProvider succeeds (0x%08lx)", hr );
    CHECK( provider == generic_provider, "typed and generic save provider results agree" );
    if (provider)
    {
        XAsyncBlock quota_async = {0};
        INT64 generic_quota = -1, typed_quota = -1;
        hr = IXGameSaveImpl3_XGameSaveGetRemainingQuotaAsync( gs, provider, &quota_async );
        CHECK( SUCCEEDED(hr), "remaining quota starts asynchronously" );
        hr = IXThreadingImpl_XAsyncGetResultSize( threading, &quota_async, &result_size );
        CHECK( SUCCEEDED(hr) && result_size == sizeof(INT64), "quota result advertises an INT64" );
        hr = IXThreadingImpl_XAsyncGetResult( threading, &quota_async, NULL, sizeof(generic_quota),
                                            &generic_quota, &used );
        CHECK( SUCCEEDED(hr) && generic_quota >= 0 && used == sizeof(generic_quota),
               "generic async retrieval returns quota bytes" );
        hr = IXGameSaveImpl3_XGameSaveGetRemainingQuotaResult( gs, &quota_async, &typed_quota );
        CHECK( SUCCEEDED(hr) && typed_quota == generic_quota, "typed and generic quota results agree" );
    }

    if (!provider) return;

    IXGameSaveImpl3_XGameSaveGetRemainingQuota( gs, provider, &quota_before );

    /* invalid container name is rejected */
    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, "bad name!", &container );
    CHECK( hr == E_GS_INVALID_CONTAINER_NAME, "invalid container name rejected (0x%08lx)", hr );

    /* create + write */
    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, "unit_test_container", &container );
    CHECK( SUCCEEDED( hr ), "CreateContainer succeeds (0x%08lx)", hr );
    hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, "Unit Test", &update );
    CHECK( SUCCEEDED( hr ), "CreateUpdate succeeds (0x%08lx)", hr );
    hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, BLOB_NAME, blob_payload, sizeof(blob_payload) );
    CHECK( SUCCEEDED( hr ), "SubmitBlobWrite succeeds (0x%08lx)", hr );
    hr = IXGameSaveImpl3_XGameSaveSubmitUpdate( gs, update );
    CHECK( SUCCEEDED( hr ), "SubmitUpdate succeeds (0x%08lx)", hr );
    IXGameSaveImpl3_XGameSaveCloseUpdate( gs, update );

    /* enumerate: the container is present with one blob */
    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, &blob_count, find_container );
    CHECK( blob_count == 1, "enumerated container has 1 blob (%d)", blob_count );

    /* read back and verify the bytes survived the round-trip */
    blobs = malloc( sizeof(XGameSaveBlob) + sizeof(blob_payload) + 64 );
    count = 1;
    hr = IXGameSaveImpl3_XGameSaveReadBlobData( gs, container, NULL, &count,
            sizeof(XGameSaveBlob) + sizeof(blob_payload) + 64, blobs );
    CHECK( SUCCEEDED( hr ) && count == 1, "ReadBlobData returns 1 blob (0x%08lx, %u)", hr, count );
    if (SUCCEEDED( hr ) && count == 1)
    {
        CHECK( blobs[0].info.size == sizeof(blob_payload), "blob size preserved (%u)", blobs[0].info.size );
        CHECK( hash_bytes( blobs[0].data, blobs[0].info.size ) == blob_expected_hash,
               "blob bytes match after round-trip" );
        CHECK( !strcmp( blobs[0].info.name, BLOB_NAME ), "blob name preserved ('%s')", blobs[0].info.name );
    }

    /* buffer too small must report E_GS_PROVIDED_BUFFER_TOO_SMALL, not overrun */
    count = 1;
    hr = IXGameSaveImpl3_XGameSaveReadBlobData( gs, container, NULL, &count, 16, blobs );
    CHECK( hr == E_GS_PROVIDED_BUFFER_TOO_SMALL, "tiny read buffer rejected (0x%08lx)", hr );
    free( blobs );

    /* quota decreased by roughly the blob size */
    IXGameSaveImpl3_XGameSaveGetRemainingQuota( gs, provider, &quota_after );
    CHECK( quota_after < quota_before, "remaining quota decreased after write (%lld -> %lld)",
           (long long)quota_before, (long long)quota_after );

    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );

    /* delete removes it */
    hr = IXGameSaveImpl3_XGameSaveDeleteContainer( gs, provider, "unit_test_container" );
    CHECK( SUCCEEDED( hr ), "DeleteContainer succeeds (0x%08lx)", hr );
    blob_count = -1;
    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, &blob_count, find_container );
    CHECK( blob_count == -1, "container is gone after delete" );

    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
}

/* ---- XStore ------------------------------------------------------------- */

static BOOLEAN __stdcall count_product( const XStoreProduct *product, void *context )
{
    (*(int *)context)++;
    return TRUE;
}

struct retained_product
{
    const XStoreProduct *product;
    const char *store_id;
    char expected_id[64];
};

static BOOLEAN CALLBACK retain_product( const XStoreProduct *product, void *context )
{
    struct retained_product *retained = context;
    retained->product = product;
    retained->store_id = product->storeId;
    snprintf( retained->expected_id, sizeof(retained->expected_id), "%s", product->storeId );
    return TRUE;
}

static volatile unsigned int scrub_sum;
static void __attribute__((noinline)) scrub_callback_stack(void)
{
    volatile BYTE scratch[4096];
    unsigned int i, sum = 0;
    for (i = 0; i < sizeof(scratch); ++i) { scratch[i] = 0x5a; sum += scratch[i]; }
    scrub_sum = sum;
}

static void test_store_product_lifetime(void)
{
    XStoreContextHandle context = NULL;
    XStoreProductQueryHandle query = NULL, other = NULL;
    XAsyncBlock block = {0};
    struct retained_product retained = {0}, second = {0};
    HRESULT hr;
    IXStoreImpl6_XStoreCreateContext( store, NULL, &context );
    hr = IXStoreImpl6_XStoreQueryProductForCurrentGameAsync( store, context, &block );
    if (hr == S_OK) hr = IXStoreImpl6_XStoreQueryProductForCurrentGameResult( store, &block, &query );
    CHECK( hr == S_OK && query, "create a current-product query for lifetime checks" );
    if (query)
    {
        hr = IXStoreImpl6_XStoreEnumerateProductsQuery( store, query, &retained, retain_product );
        CHECK( hr == S_OK && retained.product, "enumeration exposes an owned product record" );
        scrub_callback_stack();
        if (retained.product)
        {
            BOOL stable = retained.product->storeId == retained.store_id;
            CHECK( stable && !strncmp(retained.store_id, retained.expected_id, sizeof(retained.expected_id)),
                   "product and identity survive callback stack reuse" );
            memset( &block, 0, sizeof(block) );
            IXStoreImpl6_XStoreQueryProductForCurrentGameAsync( store, context, &block );
            IXStoreImpl6_XStoreQueryProductForCurrentGameResult( store, &block, &other );
            IXStoreImpl6_XStoreEnumerateProductsQuery( store, other, &second, retain_product );
            IXStoreImpl6_XStoreCloseProductsQueryHandle( store, other );
            scrub_callback_stack();
            stable = retained.product->storeId == retained.store_id;
            CHECK( stable && !strncmp(retained.store_id, retained.expected_id, sizeof(retained.expected_id)) &&
                   retained.product->productKind == XStoreProductKind_Game && retained.product->skusCount == 0,
                   "closing another query preserves the retained product until its own query closes" );
        }
        IXStoreImpl6_XStoreCloseProductsQueryHandle( store, query );
    }
    IXStoreImpl6_XStoreCloseContextHandle( store, context );
}

static void test_xstore(void)
{
    XStoreContextHandle ctx = NULL;
    XStoreLicenseHandle lic = NULL;
    XStoreProductQueryHandle query = NULL;
    XStoreGameLicense license;
    XTaskQueueRegistrationToken token = { 0 };
    XAsyncBlock async;
    SIZE_T size = 0;
    UINT32 addons = 99;
    int products = 0;
    char buf[256];
    HRESULT hr;

    printf( "# XStore\n" );
    hr = IXStoreImpl6_XStoreCreateContext( store, user, &ctx );
    CHECK( SUCCEEDED( hr ) && ctx, "CreateContext returns a handle (0x%08lx)", hr );
    if (!ctx) return;

    /* the game licence: owned, active, not a trial */
    memset( &async, 0, sizeof(async) );
    IXStoreImpl6_XStoreQueryGameLicenseAsync( store, ctx, &async );
    memset( &license, 0xcc, sizeof(license) );
    hr = IXStoreImpl6_XStoreQueryGameLicenseResult( store, &async, &license );
    CHECK( SUCCEEDED( hr ), "QueryGameLicense succeeds (0x%08lx)", hr );
    CHECK( license.isActive, "game licence isActive" );
    CHECK( !license.isTrial, "game licence is not a trial" );
    CHECK( license.skuStoreId[0] != 0, "game licence has a sku id ('%s')", license.skuStoreId );

    /* no add-ons */
    memset( &async, 0, sizeof(async) );
    IXStoreImpl6_XStoreQueryAddOnLicensesAsync( store, ctx, &async );
    hr = IXStoreImpl6_XStoreQueryAddOnLicensesResultCount( store, &async, &addons );
    CHECK( SUCCEEDED( hr ) && addons == 0, "no add-on licences (%u)", addons );

    /* This exercises the actual DLL -> Rust serializer -> HTTP -> result path.
     * The only injected component is the account/signing identity. */
    {
        static const struct { const char *custom; BOOL success; } cases[] = {
            { "regression-test", TRUE }, { "{\"nonce\":\"a\\b\"}\n\t", TRUE },
            { "română", TRUE }, { "slow", TRUE }, { "timeout", FALSE },
            { "refused", FALSE }, { "empty", FALSE }, { "null", FALSE },
            { "nested", FALSE }, { "malformed", FALSE }, { "truncated", FALSE },
            { "oversize", FALSE }, { "redirect-302", FALSE },
            { "redirect-307", FALSE }, { "redirect-308", FALSE },
            { "ipc-old-service", FALSE }, { "ipc-disconnect", FALSE },
            { "ipc-truncated", FALSE }, { "ipc-oversize", FALSE }, { "ipc-stall", FALSE },
        };
        static const char expected[] = "eyJhbGciOiJSUzI1NiJ9.Zml4dHVyZS1saWNlbmNlLXRva2Vu.c2lnbmF0dXJl";
        unsigned int i, waited;
        for (i = 0; i < sizeof(cases)/sizeof(cases[0]); i++)
        {
            char id1[32] = "9TESTSTORE0ID", id2[32] = "9TESTADDON00", custom[128];
            const char *ids[] = { id1, id2 };
            ULONGLONG started, elapsed;
            strcpy(custom, cases[i].custom);
            memset(&async, 0, sizeof(async));
            started = GetTickCount64();
            hr = IXStoreImpl6_XStoreQueryLicenseTokenAsync(store, ctx, ids, 2, custom, &async);
            elapsed = GetTickCount64() - started;
            CHECK(hr == S_OK, "license case %u starts (0x%08lx)", i, hr);
            /* Inputs may cease to exist immediately after the API returns. */
            memset(custom, '!', sizeof(custom)); memset(id1, '!', sizeof(id1)); memset(id2, '!', sizeof(id2));
            if (!strcmp(cases[i].custom, "slow"))
                CHECK(elapsed < 250, "license call returns before the 500-ms HTTP reply (%llu ms)", elapsed);
            if (FAILED(hr)) continue;
            for (waited = 0; waited < 6000; waited++)
            {
                hr = IXThreadingImpl_XAsyncGetStatus(threading, &async, FALSE);
                if (hr != E_PENDING) break;
                Sleep(10);
            }
            CHECK(hr != E_PENDING, "license case %u completes", i);
            if (!strcmp(cases[i].custom, "ipc-stall"))
                CHECK(hr == HRESULT_FROM_WIN32(ERROR_TIMEOUT) && GetTickCount64() - started >= 45000 &&
                      GetTickCount64() - started < 55000, "stalled IPC fails at its 50-second deadline");
            hr = IXStoreImpl6_XStoreQueryLicenseTokenResultSize(store, &async, &size);
            if (!cases[i].success)
            {
                CHECK(FAILED(hr) && hr != E_PENDING, "license case %u refuses bad response (0x%08lx)", i, hr);
                continue;
            }
            CHECK(hr == S_OK && size == sizeof(expected), "license case %u has exact token size", i);
            memset(buf, '!', sizeof(buf));
            hr = IXStoreImpl6_XStoreQueryLicenseTokenResult(store, &async, 4, buf);
            CHECK(hr == HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER) && buf[0] == '!', "short buffer is not written or consumed");
            hr = IXStoreImpl6_XStoreQueryLicenseTokenResult(store, &async, sizeof(buf), buf);
            CHECK(hr == S_OK && !strcmp(buf, expected) && buf[sizeof(expected)] == '!', "license case %u returns only the token and terminator", i);
        }
    }

    /* entitled products: an empty, enumerable, single-page query */
    memset( &async, 0, sizeof(async) );
    IXStoreImpl6_XStoreQueryEntitledProductsAsync( store, ctx, XStoreProductKind_Durable, 25, &async );
    hr = IXStoreImpl6_XStoreQueryEntitledProductsResult( store, &async, &query );
    CHECK( SUCCEEDED( hr ) && query, "QueryEntitledProducts returns a query (0x%08lx)", hr );
    if (query)
    {
        hr = IXStoreImpl6_XStoreEnumerateProductsQuery( store, query, &products, count_product );
        CHECK( SUCCEEDED( hr ) && products == 0, "no entitled products enumerated (%d)", products );
        CHECK( !IXStoreImpl6_XStoreProductsQueryHasMorePages( store, query ), "query has no more pages" );
    }

    /* licence-changed registration hands back a token, never fires */
    hr = IXStoreImpl6_XStoreRegisterGameLicenseChanged( store, ctx, NULL, NULL, NULL, &token );
    CHECK( SUCCEEDED( hr ), "RegisterGameLicenseChanged succeeds (0x%08lx)", hr );

    /* per-package licence acquire + validity */
    memset( &async, 0, sizeof(async) );
    IXStoreImpl6_XStoreAcquireLicenseForPackageAsync( store, query, "pkg", &async );
    hr = IXStoreImpl6_XStoreAcquireLicenseForPackageResult( store, &async, &lic );
    CHECK( SUCCEEDED( hr ) && lic, "AcquireLicenseForPackage returns a handle (0x%08lx)", hr );
    CHECK( IXStoreImpl6_XStoreIsLicenseValid( store, lic ), "acquired licence is valid" );
    IXStoreImpl6_XStoreCloseLicenseHandle( store, lic );

    IXStoreImpl6_XStoreCloseContextHandle( store, ctx );
}

/* ---- XGameUI ------------------------------------------------------------ */

static void test_xgameui(void)
{
    XAsyncBlock async;
    XGameUiMessageDialogButton button = 99;
    XGameUiTextEntryHandle entry = NULL;
    UINT32 pickers = 99, textsize = 0;
    HRESULT hr;

    printf( "# XGameUI\n" );

    /* a message dialog dismisses to its default button, without a real UI */
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogAsync( gameui, &async, "Title", "Body",
            "OK", NULL, NULL, XGameUiMessageDialogButton_Second, XGameUiMessageDialogButton_First );
    CHECK( SUCCEEDED( hr ), "ShowMessageDialogAsync succeeds (0x%08lx)", hr );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogResult( gameui, &async, &button );
    CHECK( SUCCEEDED( hr ) && button == XGameUiMessageDialogButton_Second,
           "message dialog returns the default button (%d)", button );

    /* an error dialog just dismisses */
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowErrorDialogAsync( gameui, &async, E_FAIL, "ctx" );
    hr = IXGameUiImpl4_XGameUiShowErrorDialogResult( gameui, &async );
    CHECK( SUCCEEDED( hr ), "ShowErrorDialog dismisses (0x%08lx)", hr );

    /* text entry is cancelled -> empty */
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowTextEntryAsync( gameui, &async, "T", "D", "", 0, 32 );
    hr = IXGameUiImpl4_XGameUiShowTextEntryResultSize( gameui, &async, &textsize );
    CHECK( SUCCEEDED( hr ) && textsize >= 1, "text entry result size >= 1 (%u)", textsize );

    /* player picker returns nobody */
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowPlayerPickerAsync( gameui, &async, user, "pick", 0, NULL, 0, NULL, 0, 1 );
    hr = IXGameUiImpl4_XGameUiShowPlayerPickerResultCount( gameui, &async, &pickers );
    CHECK( SUCCEEDED( hr ) && pickers == 0, "player picker returns 0 players (%u)", pickers );

    /* the on-screen text entry handle opens and closes */
    hr = IXGameUiImpl4_XGameUiTextEntryOpen( gameui, NULL, 32, "", 0, &entry );
    CHECK( SUCCEEDED( hr ) && entry, "TextEntryOpen returns a handle (0x%08lx)", hr );
    if (entry) IXGameUiImpl4_XGameUiTextEntryClose( gameui, entry );
}

/* ---- XPersistentLocalStorage + XSystem ---------------------------------- */

static void test_storage_and_system(void)
{
    HRESULT (WINAPI *query)( REFCLSID, REFIID, void ** ) = g_query;
    IXPersistentLocalStorageImpl3 *pls = NULL;
    IXSystemImpl5 *sys = NULL;
    XPersistentLocalStorageSpaceInfo space;
    char buf[512];
    SIZE_T size = 0, used = 0;
    HRESULT hr;

    printf( "# XPersistentLocalStorage / XSystem\n" );
    if (SUCCEEDED( query( &CLSID_XPersistentLocalStorageImpl, &IID_IXPersistentLocalStorageImpl3, (void **)&pls ) ))
    {
        hr = IXPersistentLocalStorageImpl3_XPersistentLocalStorageGetPathSize( pls, &size );
        CHECK( SUCCEEDED( hr ) && size > 1, "PersistentLocalStorage path size > 1 (%zu)", (size_t)size );
        hr = IXPersistentLocalStorageImpl3_XPersistentLocalStorageGetPath( pls, sizeof(buf), buf, &used );
        CHECK( SUCCEEDED( hr ) && buf[0], "PersistentLocalStorage path returned ('%s')", buf );
        memset( &space, 0, sizeof(space) );
        hr = IXPersistentLocalStorageImpl3_XPersistentLocalStorageGetSpaceInfo( pls, &space );
        CHECK( SUCCEEDED( hr ) && space.availableFreeBytes > 0, "storage reports free space" );
    }
    else CHECK( 0, "query XPersistentLocalStorage" );

    if (SUCCEEDED( query( &CLSID_XSystemImpl, &IID_IXSystemImpl5, (void **)&sys ) ))
    {
        hr = IXSystemImpl5_XSystemGetXboxLiveSandboxId( sys, sizeof(buf), buf, &used );
        CHECK( SUCCEEDED( hr ) && !strcmp( buf, "RETAIL" ), "sandbox id is RETAIL ('%s')", buf );
        hr = IXSystemImpl5_XSystemGetConsoleId( sys, sizeof(buf), buf, &used );
        CHECK( SUCCEEDED( hr ) && buf[0], "console id returned" );
    }
    else CHECK( 0, "query XSystem" );

    {
        IXSystemAnalyticsImpl *analytics = NULL;
        struct { UINT64 before; XSystemAnalyticsInfo info; UINT64 after; } guarded;
        HMODULE combase = LoadLibraryA( "combase.dll" );
        HRESULT (WINAPI *ro_init)( UINT32 ) = (void *)GetProcAddress( combase, "RoInitialize" );
        void (WINAPI *ro_uninit)( void ) = (void *)GetProcAddress( combase, "RoUninitialize" );
        unsigned int iteration;
        BOOL initialized = FALSE;

        hr = query( &CLSID_XSystemAnalyticsImpl, &IID_IXSystemAnalyticsImpl, (void **)&analytics );
        CHECK( hr == S_OK && analytics, "query system analytics interface" );
        if (analytics)
        {
            for (iteration = 0; iteration < 2; ++iteration)
            {
                XSystemAnalyticsInfo *result;
                if (iteration && ro_init) initialized = SUCCEEDED( ro_init( 1 ) );
                memset( &guarded, 0xa5, sizeof(guarded) );
                result = analytics->lpVtbl->XSystemGetAnalyticsInfo( analytics, &guarded.info );
                CHECK( result == &guarded.info && guarded.before == 0xa5a5a5a5a5a5a5a5ULL &&
                       guarded.after == 0xa5a5a5a5a5a5a5a5ULL, "analytics preserves its aggregate return buffer %u", iteration );
                CHECK( !strcmp( guarded.info.family, "Windows.Desktop" ) &&
                       !strcmp( guarded.info.form, "Desktop" ),
                       "analytics reports the complete family accepted by native Party %u", iteration );
                CHECK( guarded.info.osVersion.major &&
                       !memcmp( &guarded.info.osVersion, &guarded.info.hostingOsVersion, sizeof(XVersion) ),
                       "analytics returns initialized, consistent OS versions %u", iteration );
                if (initialized && ro_uninit) ro_uninit();
            }
            IXSystemAnalyticsImpl_Release( analytics );
        }
        if (combase) FreeLibrary( combase );
    }

    {
        IXNetworkingImpl2 *net = NULL;
        UINT16 port = 0;
        if (SUCCEEDED( query( &CLSID_XNetworkingImpl, &IID_IXNetworkingImpl2, (void **)&net ) ))
        {
            hr = IXNetworkingImpl2_XNetworkingQueryPreferredLocalUdpMultiplayerPort( net, &port );
            CHECK( SUCCEEDED( hr ) && port != 0, "preferred UDP multiplayer port (%u)", port );

            /* libHttpClient asks for a URL's security requirements before it
             * opens a WebSocket -- rta.xboxlive.com and the multiplayer
             * signalling host among them. This used to return E_NOTIMPL and
             * never complete the block, and libHttpClient's connect-failure
             * teardown then destroyed the WebSocket while other threads still
             * held it: a thread parked for ever on its freed mutex, which is
             * what froze a server join from the in-game menu. */
            {
                XAsyncBlock async = { 0 };
                XNetworkingSecurityInformation *info = NULL;
                SIZE_T size = 0, used = 0;
                UINT8 buffer[256];
                int waited;

                hr = IXNetworkingImpl2_XNetworkingQuerySecurityInformationForUrlAsync(
                        net, "wss://rta.xboxlive.com/connect", &async );
                CHECK( hr == S_OK, "QuerySecurityInformationForUrlAsync is accepted (0x%08lx)", hr );

                for (waited = 0; waited < 200; waited++)
                {
                    hr = IXNetworkingImpl2_XNetworkingQuerySecurityInformationForUrlAsyncResultSize( net, &async, &size );
                    if (hr != E_PENDING) break;
                    Sleep( 10 );
                }
                CHECK( hr == S_OK && size >= sizeof(XNetworkingSecurityInformation),
                       "the operation completes and reports a result size (0x%08lx, %Iu)", hr, size );

                hr = IXNetworkingImpl2_XNetworkingQuerySecurityInformationForUrlAsyncResult(
                        net, &async, sizeof(buffer), &used, buffer, &info );
                CHECK( hr == S_OK && info != NULL, "the security information is returned (0x%08lx, %p)", hr, info );
                if (hr == S_OK && info)
                    CHECK( info->thumbprintCount == 0 && info->thumbprints == NULL,
                           "no certificate thumbprints are pinned (%Iu, %p)",
                           info->thumbprintCount, info->thumbprints );

                /* A buffer that is too small must be reported, not overrun. */
                hr = IXNetworkingImpl2_XNetworkingQuerySecurityInformationForUrlAsyncResult(
                        net, &async, 1, &used, buffer, &info );
                CHECK( hr == HRESULT_FROM_WIN32( ERROR_INSUFFICIENT_BUFFER ),
                       "an undersized buffer is rejected (0x%08lx)", hr );
            }
        }
        else CHECK( 0, "query XNetworking" );
    }
}

/* ---- XThreading --------------------------------------------------------- */

/* Which operation the runtime drove, for the reused-block check below. */
static volatile LONG which_provider_ran;

static HRESULT CALLBACK provider_a( XAsyncOp op, const XAsyncProviderData *data )
{
    if (op == XAsyncOp_DoWork) InterlockedExchange( (LONG *)&which_provider_ran, 1 );
    return S_OK;
}

static HRESULT CALLBACK provider_b( XAsyncOp op, const XAsyncProviderData *data )
{
    if (op == XAsyncOp_DoWork) InterlockedExchange( (LONG *)&which_provider_ran, 2 );
    return S_OK;
}

static volatile LONG completion_ran;

static void CALLBACK completion_cb( XAsyncBlock *async )
{
    InterlockedExchange( (LONG *)&completion_ran, 1 );
}

/* A provider that finishes the operation from inside its own Begin and then
 * reports E_PENDING, which is what libHttpClient's WinHttp websocket send does
 * because Wine completes a send that does not block there and then. */
static IXThreadingImpl *threading_for_provider;
static volatile LONG inside_begin, called_back_inside_begin;

#define EARLY_RESULT 0x00c0ffee

static HRESULT CALLBACK provider_completes_in_begin( XAsyncOp op, const XAsyncProviderData *data )
{
    switch (op)
    {
    case XAsyncOp_Begin:
        InterlockedExchange( (LONG *)&inside_begin, 1 );
        IXThreadingImpl_XAsyncComplete( threading_for_provider, data->async, S_OK, sizeof(UINT32) );
        InterlockedExchange( (LONG *)&inside_begin, 0 );
        return E_PENDING;
    case XAsyncOp_GetResult:
        if (data->buffer && data->bufferSize >= sizeof(UINT32))
            *(UINT32 *)data->buffer = EARLY_RESULT;
        return S_OK;
    default:
        return S_OK;
    }
}

static volatile LONG failed_begin_cleanups, failed_begin_callback_ok, failed_begin_cleanup_context_ok;
static UINT32 failed_begin_cookie;

static HRESULT CALLBACK provider_fails_in_begin( XAsyncOp op, const XAsyncProviderData *data )
{
    if (op == XAsyncOp_Cleanup)
    {
        if (data->async->context == &failed_begin_cookie)
            InterlockedExchange( (LONG *)&failed_begin_cleanup_context_ok, 1 );
        InterlockedIncrement( (LONG *)&failed_begin_cleanups );
    }
    return op == XAsyncOp_Begin ? E_FAIL : S_OK;
}

static volatile LONG failed_work_finished, failed_work_cleanup_safe;
static HRESULT work_completion_result = E_FAIL;
static HRESULT CALLBACK provider_fails_in_work( XAsyncOp op, const XAsyncProviderData *data )
{
    switch (op)
    {
    case XAsyncOp_Begin:
        return IXThreadingImpl_XAsyncSchedule(threading_for_provider, data->async, 0);
    case XAsyncOp_DoWork:
        IXThreadingImpl_XAsyncComplete(threading_for_provider, data->async, work_completion_result, 0);
        Sleep(30); /* The callback may free the user's block before work exits. */
        InterlockedExchange((LONG *)&failed_work_finished, 1);
        return E_PENDING;
    case XAsyncOp_Cleanup:
        if (failed_work_finished) InterlockedExchange((LONG *)&failed_work_cleanup_safe, 1);
        return provider_fails_in_begin(op, data);
    default:
        return S_OK;
    }
}

static HRESULT CALLBACK provider_completes_then_fails( XAsyncOp op, const XAsyncProviderData *data )
{
    HRESULT hr = provider_completes_in_begin( op, data );
    return op == XAsyncOp_Begin ? E_FAIL : hr;
}

static HRESULT CALLBACK provider_completes_without_payload( XAsyncOp op, const XAsyncProviderData *data )
{
    if (op == XAsyncOp_Begin)
        IXThreadingImpl_XAsyncComplete( threading_for_provider, data->async, S_OK, 0 );
    if (op == XAsyncOp_Cleanup) InterlockedIncrement((LONG *)&failed_begin_cleanups);
    return S_OK;
}

static void CALLBACK failed_begin_cb( XAsyncBlock *async )
{
    HRESULT hr = IXThreadingImpl_XAsyncGetStatus( threading_for_provider, async, FALSE );
    if (hr == work_completion_result && !failed_begin_cleanups)
        InterlockedExchange( (LONG *)&failed_begin_callback_ok, 1 );
    VirtualFree( async, 0, MEM_RELEASE );
    InterlockedExchange( (LONG *)&completion_ran, 1 );
}

static void CALLBACK early_completion_cb( XAsyncBlock *async )
{
    if (InterlockedCompareExchange( (LONG *)&inside_begin, 0, 0 ))
        InterlockedExchange( (LONG *)&called_back_inside_begin, 1 );
    InterlockedExchange( (LONG *)&completion_ran, 1 );
}

/* A task queue port runs one callback at a time. Before this was true, ten
 * threads submitting to a single queue during a server join had libHttpClient
 * callbacks running concurrently; one freed the HTTP call while another was
 * still inside it, and the game deadlocked on a std::mutex whose memory had
 * been reused for a response body. */
#define SERIAL_CALLBACKS 40

static LONG serial_inflight, serial_max_inflight, serial_ran;
static LONG serial_order[SERIAL_CALLBACKS];

static void CALLBACK serial_cb( void *context, BOOLEAN canceled )
{
    LONG now = InterlockedIncrement( &serial_inflight );
    LONG slot;

    if (now > InterlockedCompareExchange( &serial_max_inflight, 0, 0 ))
        InterlockedExchange( &serial_max_inflight, now );

    /* Widen the window: overlapping callbacks would be caught here. */
    Sleep( 1 );

    slot = InterlockedIncrement( &serial_ran ) - 1;
    if (slot >= 0 && slot < SERIAL_CALLBACKS) serial_order[slot] = (LONG)(LONG_PTR)context;
    InterlockedDecrement( &serial_inflight );
}

static void test_xthreading(void)
{
    XAsyncBlock async = { 0 };
    SIZE_T size = 0;
    HRESULT hr;

    printf( "# XThreading\n" );
    /* GetResultSize on a not-started block is E_PENDING, not a crash */
    hr = IXThreadingImpl_XAsyncGetResultSize( threading, &async, &size );
    CHECK( hr == E_PENDING || FAILED( hr ), "XAsyncGetResultSize on idle block does not succeed (0x%08lx)", hr );

    /* A null queue handle is not an error: it names the process default queue.
     * libHttpClient asks for that queue's port while setting up a request, so
     * rejecting null made XblProfileGetUserProfilesAsync fail with E_INVALIDARG
     * -- the "Profile could not be loaded" message, followed by a shutdown. */
    {
        XTaskQueueHandle proc = NULL, mine = NULL, got = NULL;
        XTaskQueuePortHandle port = NULL;

        hr = IXThreadingImpl_XTaskQueueGetPort( threading, NULL, XTaskQueuePort_Work, &port );
        CHECK( hr == S_OK && port != NULL, "XTaskQueueGetPort(null queue) succeeds (0x%08lx, port %p)", hr, port );

        CHECK( IXThreadingImpl_XTaskQueueGetCurrentProcessTaskQueue( threading, &proc ) && proc,
               "a process default task queue is published (%p)", proc );

        hr = IXThreadingImpl_XTaskQueueCreate( threading, XTaskQueueDispatchMode_Manual,
                                               XTaskQueueDispatchMode_Manual, &mine );
        CHECK( hr == S_OK && mine, "XTaskQueueCreate succeeds (0x%08lx)", hr );
        if (hr == S_OK)
        {
            IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, mine );
            IXThreadingImpl_XTaskQueueGetCurrentProcessTaskQueue( threading, &got );
            CHECK( got == mine, "a title-installed process queue wins (%p vs %p)", got, mine );
            if (got) IXThreadingImpl_XTaskQueueCloseHandle( threading, got );

            /* put the original back so later tests see a sane default */
            IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, proc );
            IXThreadingImpl_XTaskQueueCloseHandle( threading, mine );
        }
        if (proc) IXThreadingImpl_XTaskQueueCloseHandle( threading, proc );
    }

    /* Titles reuse an XAsyncBlock: Minecraft runs three XUserAddAsync calls
     * through one address. Operations are keyed by that address, so when a
     * finished one is still listed the runtime must drive the newest, not the
     * stale entry -- otherwise the new operation is begun, scheduled and never
     * run, which is what left the profile screen waiting. */
    {
        /* Deliberately on the heap and never freed: the operations begun here
         * stay registered against this address, and a stack block would let the
         * compiler hand the same slot to a later test, whose own operations
         * would then collide with these -- the very confusion under test. */
        XAsyncBlock *shared = calloc( 1, sizeof(*shared) );
        int waited;

        InterlockedExchange( (LONG *)&which_provider_ran, 0 );

        hr = IXThreadingImpl_XAsyncBegin( threading, shared, NULL, NULL, "older", provider_a );
        CHECK( hr == S_OK, "XAsyncBegin for the first operation (0x%08lx)", hr );
        hr = IXThreadingImpl_XAsyncBegin( threading, shared, NULL, NULL, "newer", provider_b );
        CHECK( hr == S_OK, "XAsyncBegin reusing the same block (0x%08lx)", hr );

        hr = IXThreadingImpl_XAsyncSchedule( threading, shared, 0 );
        CHECK( hr == S_OK, "XAsyncSchedule on the reused block (0x%08lx)", hr );

        for (waited = 0; waited < 200 && !which_provider_ran; waited++) Sleep( 10 );
        CHECK( which_provider_ran == 2,
               "the newest operation for a reused block is the one driven (ran %ld)",
               which_provider_ran );
    }

    /* A completion for a block with no queue of its own must still be
     * delivered when a Manual queue is installed as the process default.
     * Bedrock does exactly that and then never calls XTaskQueueDispatch, so
     * routing these completions to the process queue's mode parked them for
     * ever: half its XUserAddAsync calls never finished and sign-in died. */
    {
        XTaskQueueHandle manual = NULL, saved = NULL;
        XAsyncBlock block;
        HRESULT status;
        int waited;

        IXThreadingImpl_XTaskQueueGetCurrentProcessTaskQueue( threading, &saved );
        hr = IXThreadingImpl_XTaskQueueCreate( threading, XTaskQueueDispatchMode_Manual,
                                               XTaskQueueDispatchMode_Manual, &manual );
        if (hr == S_OK && manual)
        {
            IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, manual );

            memset( &block, 0, sizeof(block) );
            block.queue = NULL;                    /* no queue of its own */
            block.callback = completion_cb;
            InterlockedExchange( (LONG *)&completion_ran, 0 );
            /* Begin it first. XAsyncComplete only means anything for a block
             * this runtime started, and a title always gets here through an
             * *Async() call; completing a block out of nowhere used to work
             * only because a stale operation happened to sit at the same
             * stack address. */
            IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_a, "routing", provider_a );
            IXThreadingImpl_XAsyncComplete( threading, &block, S_OK, 0 );

            /* never dispatch the manual queue -- just as the title never does.
             * The callback running is the thing that matters: the status is set
             * regardless, but a parked completion never calls back. */
            for (waited = 0; waited < 200 && !completion_ran; waited++) Sleep( 10 );
            status = IXThreadingImpl_XAsyncGetStatus( threading, &block, FALSE );
            CHECK( completion_ran,
                   "a null-queue completion callback runs despite a Manual process queue (status 0x%08lx)", status );

            /* And the same for a block that names the Manual queue itself.
             * XSAPI is handed one of the title's Manual queues, so every Xbl*
             * completion took this path and was parked: 42 operations begun in
             * one run, none finished. */
            memset( &block, 0, sizeof(block) );
            block.queue = manual;
            block.callback = completion_cb;
            InterlockedExchange( (LONG *)&completion_ran, 0 );
            IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_b, "routing", provider_b );
            IXThreadingImpl_XAsyncComplete( threading, &block, S_OK, 0 );
            for (waited = 0; waited < 200 && !completion_ran; waited++) Sleep( 10 );
            CHECK( completion_ran,
                   "a Manual-queue completion is delivered while nothing pumps the queue" );

            IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, saved );
            IXThreadingImpl_XTaskQueueCloseHandle( threading, manual );
        }
        if (saved) IXThreadingImpl_XTaskQueueCloseHandle( threading, saved );
    }

    /* A provider may finish the block from inside Begin. Two things must hold:
     * the callback waits until Begin has returned -- every GDK API promises to
     * return before its completion routine runs, and a title finishes wiring up
     * the object the callback reaches through after the call -- and the
     * operation survives Begin reporting E_PENDING, so its result is still
     * there to collect. Getting either wrong crashed Forza Horizon 5 in its
     * websocket send handler. */
    {
        XAsyncBlock block;
        UINT32 value = 0;
        int waited;

        threading_for_provider = threading;
        memset( &block, 0, sizeof(block) );
        block.callback = early_completion_cb;
        InterlockedExchange( (LONG *)&completion_ran, 0 );
        InterlockedExchange( (LONG *)&called_back_inside_begin, 0 );

        hr = IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_completes_in_begin,
                                          "completes-in-begin", provider_completes_in_begin );
        CHECK( hr == S_OK, "a provider returning E_PENDING starts successfully (0x%08lx)", hr );
        CHECK( !called_back_inside_begin,
               "a completion raised inside Begin does not call back before Begin returns" );

        for (waited = 0; waited < 200 && !completion_ran; waited++) Sleep( 10 );
        CHECK( completion_ran, "the held completion is delivered once Begin has returned" );

        hr = IXThreadingImpl_XAsyncGetResult( threading, &block, provider_completes_in_begin,
                                              sizeof(value), &value, NULL );
        CHECK( hr == S_OK && value == EARLY_RESULT,
               "the operation survives E_PENDING and still has its result (0x%08lx, %#lx)",
               hr, (unsigned long)value );
    }

    {
        XAsyncBlock *block = VirtualAlloc( NULL, sizeof(*block), MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE );
        int waited;

        CHECK( block != NULL, "allocate a disposable failed-Begin async block" );
        if (!block) return;
        block->callback = failed_begin_cb;
        block->context = &failed_begin_cookie;
        InterlockedExchange( (LONG *)&completion_ran, 0 );
        InterlockedExchange( (LONG *)&failed_begin_cleanups, 0 );
        InterlockedExchange( (LONG *)&failed_begin_callback_ok, 0 );
        InterlockedExchange( (LONG *)&failed_begin_cleanup_context_ok, 0 );
        hr = IXThreadingImpl_XAsyncBegin( threading, block, NULL, provider_fails_in_begin,
                                          "fails-in-begin", provider_fails_in_begin );
        CHECK( hr == S_OK, "provider failure is reported asynchronously after setup (0x%08lx)", hr );
        for (waited = 0; waited < 200 && !failed_begin_cleanups; waited++) Sleep( 10 );
        CHECK( completion_ran, "a failure returned by Begin delivers the completion callback" );
        CHECK( failed_begin_callback_ok, "failed Begin reports E_FAIL before cleaning its provider" );
        CHECK( failed_begin_cleanups == 1, "failed Begin cleans up exactly once without GetResult (%ld)",
               failed_begin_cleanups );
        CHECK( failed_begin_cleanup_context_ok, "Cleanup can read its async block after the callback frees the user's block" );
    }

    {
        XAsyncBlock *block = VirtualAlloc(NULL, sizeof(*block), MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        int waited;
        CHECK(block != NULL, "allocate a disposable failed-work block");
        if (!block) return;
        block->callback = failed_begin_cb;
        block->context = &failed_begin_cookie;
        InterlockedExchange((LONG *)&completion_ran, 0);
        InterlockedExchange((LONG *)&failed_begin_cleanups, 0);
        InterlockedExchange((LONG *)&failed_begin_callback_ok, 0);
        InterlockedExchange((LONG *)&failed_begin_cleanup_context_ok, 0);
        InterlockedExchange((LONG *)&failed_work_finished, 0);
        InterlockedExchange((LONG *)&failed_work_cleanup_safe, 0);
        hr = IXThreadingImpl_XAsyncBegin(threading, block, NULL, provider_fails_in_work,
                                        "fails-in-work", provider_fails_in_work);
        CHECK(hr == S_OK, "background failure starts successfully");
        for (waited = 0; waited < 200 && !failed_begin_cleanups; waited++) Sleep(10);
        CHECK(completion_ran && failed_begin_callback_ok, "background failure reports status before cleanup");
        CHECK(failed_begin_cleanups == 1 && failed_work_cleanup_safe,
              "background failure cleans up once, after worker and callback, without GetResult");
        CHECK(failed_begin_cleanup_context_ok, "background cleanup uses a saved block after callback frees the original");
    }

    {
        XAsyncBlock *block = VirtualAlloc(NULL, sizeof(*block), MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        int waited;

        work_completion_result = S_OK;
        block->context = &failed_begin_cookie;
        block->callback = failed_begin_cb;
        InterlockedExchange((LONG *)&completion_ran, 0);
        InterlockedExchange((LONG *)&failed_begin_cleanups, 0);
        InterlockedExchange((LONG *)&failed_begin_callback_ok, 0);
        InterlockedExchange((LONG *)&failed_begin_cleanup_context_ok, 0);
        InterlockedExchange((LONG *)&failed_work_finished, 0);
        InterlockedExchange((LONG *)&failed_work_cleanup_safe, 0);
        hr = IXThreadingImpl_XAsyncBegin(threading, block, NULL, provider_fails_in_work,
                                        "successful-work-without-result", provider_fails_in_work);
        CHECK(hr == S_OK, "zero-result background operation starts");
        for (waited = 0; waited < 200 && !failed_begin_cleanups; waited++) Sleep(10);
        CHECK(completion_ran && failed_begin_callback_ok, "zero-result success reports status before cleanup");
        CHECK(failed_begin_cleanups == 1 && failed_work_cleanup_safe,
              "zero-result success releases its provider once after work without GetResult");
        CHECK(failed_begin_cleanup_context_ok, "zero-result cleanup survives callback freeing the original block");
        work_completion_result = E_FAIL;
    }

    {
        XAsyncBlock block = {0};
        UINT32 value = 0;

        block.context = &failed_begin_cookie;
        InterlockedExchange( (LONG *)&failed_begin_cleanups, 0 );
        hr = IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_fails_in_begin,
                                          "failure-without-callback", provider_fails_in_begin );
        CHECK( hr == S_OK && failed_begin_cleanups == 1,
               "failed Begin without a callback releases the provider (0x%08lx, %ld)", hr, failed_begin_cleanups );
        hr = IXThreadingImpl_XAsyncGetStatus( threading, &block, FALSE );
        CHECK( hr == E_FAIL, "failure remains readable after automatic cleanup (0x%08lx)", hr );
        hr = IXThreadingImpl_XAsyncGetResult( threading, &block, provider_fails_in_begin, 0, NULL, NULL );
        CHECK( hr == E_FAIL, "failed result remains readable after automatic cleanup (0x%08lx)", hr );

        memset( &block, 0, sizeof(block) );
        InterlockedExchange((LONG *)&failed_begin_cleanups, 0);
        IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_completes_without_payload,
                                     "older-zero-payload", provider_completes_without_payload );
        CHECK(failed_begin_cleanups == 1, "inline zero-result success cleans up without a callback or result read");
        {
            SIZE_T used = 99;
            hr = IXThreadingImpl_XAsyncGetResult(threading, &block, provider_completes_without_payload, 0, NULL, &used);
            CHECK(hr == S_OK && used == 0, "detached zero-result success remains readable with zero bytes used");
        }
        block.context = &failed_begin_cookie;
        IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_fails_in_begin,
                                     "newer-failed-begin", provider_fails_in_begin );
        hr = IXThreadingImpl_XAsyncGetResult( threading, &block, provider_fails_in_begin, 0, NULL, NULL );
        CHECK( hr == E_FAIL, "a reused block returns its new Begin failure instead of an older result (0x%08lx)", hr );

        IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_b, "pending-after-failure", provider_b );
        hr = IXThreadingImpl_XAsyncGetStatus( threading, &block, FALSE );
        CHECK( hr == E_PENDING, "restarting a failed block clears its old terminal status (0x%08lx)", hr );
        IXThreadingImpl_XAsyncComplete( threading, &block, S_OK, 0 );
        IXThreadingImpl_XAsyncGetResult( threading, &block, provider_b, 0, NULL, NULL );

        memset( &block, 0, sizeof(block) );
        hr = IXThreadingImpl_XAsyncBegin( threading, &block, NULL, provider_completes_then_fails,
                                          "success-before-begin-failure", provider_completes_then_fails );
        CHECK( hr == S_OK, "inline completion followed by Begin failure still starts successfully (0x%08lx)", hr );
        hr = IXThreadingImpl_XAsyncGetResult( threading, &block, provider_completes_then_fails,
                                              sizeof(value), &value, NULL );
        CHECK( hr == S_OK && value == EARLY_RESULT,
               "an inline result wins over a later Begin failure (0x%08lx, %#lx)", hr, (unsigned long)value );
    }

    {
        XTaskQueueHandle queue = NULL;
        int waited, ordered = 1, i;

        hr = IXThreadingImpl_XTaskQueueCreate( threading, XTaskQueueDispatchMode_Manual,
                                               XTaskQueueDispatchMode_Manual, &queue );
        CHECK( hr == S_OK && queue, "XTaskQueueCreate for the serialisation test succeeds (0x%08lx)", hr );
        if (hr == S_OK)
        {
            InterlockedExchange( &serial_inflight, 0 );
            InterlockedExchange( &serial_max_inflight, 0 );
            InterlockedExchange( &serial_ran, 0 );

            for (i = 0; i < SERIAL_CALLBACKS; i++)
            {
                /* Mix plain and delayed submissions: a delayed callback joins
                 * the same queue when its delay expires, so it must be run in
                 * isolation too. */
                if (i % 4 == 3)
                    hr = IXThreadingImpl_XTaskQueueSubmitDelayedCallback( threading, queue, XTaskQueuePort_Work,
                                                                          1, (void *)(LONG_PTR)i, serial_cb );
                else
                    hr = IXThreadingImpl_XTaskQueueSubmitCallback( threading, queue, XTaskQueuePort_Work,
                                                                   (void *)(LONG_PTR)i, serial_cb );
                if (FAILED(hr)) break;
            }
            CHECK( SUCCEEDED(hr), "submitting %d callbacks to one queue succeeds (0x%08lx)", SERIAL_CALLBACKS, hr );

            for (waited = 0; waited < 600 && serial_ran < SERIAL_CALLBACKS; waited++) Sleep( 10 );

            CHECK( serial_ran == SERIAL_CALLBACKS, "every queued callback runs (%ld of %d)",
                   serial_ran, SERIAL_CALLBACKS );
            CHECK( serial_max_inflight == 1, "a queue never runs two callbacks at once (peak %ld)",
                   serial_max_inflight );

            /* Submissions without a delay must also keep their order. */
            for (i = 1; i < SERIAL_CALLBACKS; i++)
                if (serial_order[i] % 4 != 3 && serial_order[i - 1] % 4 != 3
                    && serial_order[i] < serial_order[i - 1]) ordered = 0;
            CHECK( ordered, "undelayed callbacks run in the order they were submitted" );

            IXThreadingImpl_XTaskQueueCloseHandle( threading, queue );
        }
    }
}


struct manual_probe
{
    LONG work_thread, completion_thread, cleaned, direct_thread;
};

static HRESULT CALLBACK manual_provider( XAsyncOp operation, const XAsyncProviderData *data )
{
    struct manual_probe *probe = data->context;
    if (operation == XAsyncOp_DoWork)
    {
        InterlockedExchange( &probe->work_thread, GetCurrentThreadId() );
        IXThreadingImpl_XAsyncComplete( threading, data->async, S_OK, sizeof(UINT32) );
        return E_PENDING;
    }
    if (operation == XAsyncOp_GetResult) *(UINT32 *)data->buffer = 0x11223344;
    if (operation == XAsyncOp_Cleanup) InterlockedIncrement( &probe->cleaned );
    return S_OK;
}

static void CALLBACK manual_completed( XAsyncBlock *block )
{
    struct manual_probe *probe = block->context;
    InterlockedExchange( &probe->completion_thread, GetCurrentThreadId() );
}

static void CALLBACK manual_direct( void *context, BOOLEAN canceled )
{
    struct manual_probe *probe = context;
    InterlockedExchange( &probe->direct_thread, canceled ? -1 : GetCurrentThreadId() );
}

static void test_manual_queues(void)
{
    XTaskQueueHandle parent = NULL, other = NULL, composite = NULL, saved = NULL;
    XTaskQueuePortHandle work_port = NULL, completion_port = NULL;
    struct manual_probe probe = {0}, retained = {0};
    XAsyncBlock block = {0};
    DWORD owner = GetCurrentThreadId();
    UINT32 value = 0;
    HRESULT hr;
    unsigned int iteration;

    printf( "# Manual queue thread, port, and composite routing\n" );
    hr = IXThreadingImpl_XTaskQueueCreate( threading, XTaskQueueDispatchMode_Manual,
                                          XTaskQueueDispatchMode_Manual, &parent );
    CHECK( hr == S_OK && parent, "create the caller-dispatched queue" );
    hr = IXThreadingImpl_XTaskQueueCreate( threading, XTaskQueueDispatchMode_Manual,
                                          XTaskQueueDispatchMode_Manual, &other );
    CHECK( hr == S_OK && other, "create a distinct queue" );
    if (!parent || !other) return;
    IXThreadingImpl_XTaskQueueGetCurrentProcessTaskQueue( threading, &saved );

    for (iteration = 0; iteration < 4; ++iteration)
    {
        memset( &probe, 0, sizeof(probe) );
        memset( &block, 0, sizeof(block) );
        block.context = &probe;
        block.callback = manual_completed;
        block.queue = parent;
        if (iteration == 1)
        {
            IXThreadingImpl_XTaskQueueGetPort( threading, parent, XTaskQueuePort_Work, &work_port );
            IXThreadingImpl_XTaskQueueGetPort( threading, other, XTaskQueuePort_Completion, &completion_port );
            hr = IXThreadingImpl_XTaskQueueCreateComposite( threading, work_port, completion_port, &composite );
            CHECK( hr == S_OK && composite, "composite retains the original port identities" );
            block.queue = composite;
        }
        if (iteration == 3)
        {
            IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, parent );
            block.queue = NULL;
        }
        hr = IXThreadingImpl_XAsyncBegin( threading, &block, &probe, manual_provider, "manual-probe", manual_provider );
        CHECK( hr == S_OK, "begin manual operation %u", iteration );
        hr = IXThreadingImpl_XAsyncSchedule( threading, &block, iteration == 2 ? 10 : 0 );
        CHECK( hr == S_OK, "schedule manual operation %u", iteration );
        Sleep( 40 );
        CHECK( !probe.work_thread && !probe.completion_thread,
               "operation %u stays queued until the caller dispatches", iteration );
        IXThreadingImpl_XTaskQueueDispatch( threading, other, XTaskQueuePort_Work, 0 );
        IXThreadingImpl_XTaskQueueDispatch( threading, parent, XTaskQueuePort_Completion, 0 );
        CHECK( !probe.work_thread, "the wrong queue and port cannot run work %u", iteration );
        IXThreadingImpl_XTaskQueueDispatch( threading, parent, XTaskQueuePort_Work, 0 );
        CHECK( probe.work_thread == owner && !probe.completion_thread,
               "work %u runs on the dispatching thread and leaves completion queued", iteration );
        IXThreadingImpl_XTaskQueueDispatch( threading, iteration == 1 ? parent : other,
                                          XTaskQueuePort_Completion, 0 );
        CHECK( !probe.completion_thread, "the wrong completion port cannot run callback %u", iteration );
        IXThreadingImpl_XTaskQueueDispatch( threading, iteration == 1 ? other : parent,
                                          XTaskQueuePort_Completion, 0 );
        CHECK( probe.completion_thread == owner, "completion %u runs on the owning dispatch thread", iteration );
        hr = IXThreadingImpl_XAsyncGetResult( threading, &block, manual_provider, sizeof(value), &value, NULL );
        CHECK( hr == S_OK && value == 0x11223344 && probe.cleaned == 1,
               "operation %u returns its result and cleans up exactly once", iteration );
    }
    IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, saved );
    memset( &probe, 0, sizeof(probe) );
    hr = IXThreadingImpl_XTaskQueueSubmitCallback( threading, parent, XTaskQueuePort_Work, &probe, manual_direct );
    Sleep( 30 );
    CHECK( hr == S_OK && !probe.direct_thread, "a direct callback also waits for manual dispatch" );
    IXThreadingImpl_XTaskQueueDispatch( threading, parent, XTaskQueuePort_Work, 0 );
    CHECK( probe.direct_thread == owner, "a direct callback runs on the registered caller thread" );
    memset( &probe, 0, sizeof(probe) );
    hr = IXThreadingImpl_XTaskQueueSubmitDelayedCallback( threading, other, XTaskQueuePort_Work,
                                                         1000, &probe, manual_direct );
    CHECK( hr == S_OK, "accept delayed manual work" );
    CHECK( !IXThreadingImpl_XTaskQueueDispatch( threading, other, XTaskQueuePort_Work, 0 ) &&
           !probe.direct_thread, "manual work cannot run before its delay" );
    hr = IXThreadingImpl_XTaskQueueTerminate( threading, other, TRUE, NULL, NULL );
    CHECK( hr == S_OK && probe.direct_thread == -1,
           "termination cancels delayed manual work before returning" );
    probe.direct_thread = 0;
    Sleep( 1100 );
    CHECK( !IXThreadingImpl_XTaskQueueDispatch( threading, other, XTaskQueuePort_Work, 0 ) &&
           !probe.direct_thread, "no timer can deliver work after termination" );
    hr = IXThreadingImpl_XTaskQueueSubmitDelayedCallback( threading, other, XTaskQueuePort_Work,
                                                         1, &probe, manual_direct );
    CHECK( hr == E_ABORT, "a terminated queue rejects delayed work" );
    if (composite)
    {
        memset( &probe, 0, sizeof(probe) );
        hr = IXThreadingImpl_XTaskQueueSubmitCallback( threading, parent, XTaskQueuePort_Work,
                                                      &retained, manual_direct );
        CHECK( hr == S_OK, "queue parent work on a shared endpoint" );
        hr = IXThreadingImpl_XTaskQueueSubmitDelayedCallback( threading, composite, XTaskQueuePort_Work,
                                                             1000, &probe, manual_direct );
        CHECK( hr == S_OK, "queue composite work on the shared endpoint" );
        hr = IXThreadingImpl_XTaskQueueTerminate( threading, composite, TRUE, NULL, NULL );
        CHECK( hr == S_OK && probe.direct_thread == -1 && !retained.direct_thread,
               "terminating a composite cancels only its own callbacks" );
        IXThreadingImpl_XTaskQueueDispatch( threading, parent, XTaskQueuePort_Work, 0 );
        CHECK( retained.direct_thread == owner, "parent work survives composite termination" );
    }
    if (composite) IXThreadingImpl_XTaskQueueCloseHandle( threading, composite );
    if (saved) IXThreadingImpl_XTaskQueueCloseHandle( threading, saved );
    IXThreadingImpl_XTaskQueueCloseHandle( threading, other );
    IXThreadingImpl_XTaskQueueCloseHandle( threading, parent );
}

static void test_gamesave_interface4( const char *scid )
{
    IXGameSaveImpl4 *modern = NULL;
    IXGameSaveImpl3 *legacy = NULL;
    PFXGameSaveConfigHandle config = (PFXGameSaveConfigHandle)0x1234;
    XGameSaveProviderHandle provider = NULL;
    XAsyncBlock async = {0};
    SIZE_T size = 0;
    INT64 quota = -1;
    UINT32 state = 0;
    UINT64 current = 0, total = 0;
    char *folder = NULL, filename[MAX_PATH], readback[8] = {0};
    HANDLE file;
    DWORD written = 0, read = 0;
    HRESULT hr;

    printf("# Modern SDK save interface: legacy APIs and explicit cloud failure\n");
    hr = g_query(&CLSID_XGameSaveImpl, &IID_IXGameSaveImpl4, (void **)&modern);
    CHECK(SUCCEEDED(hr) && modern, "new save interface is available (%#lx)", hr);
    if (!modern) return;
    CHECK(sizeof(IXGameSaveImpl4Vtbl) == 50 * sizeof(void *), "full GDK 2510 fifty-slot vtable");
    hr = IXGameSaveImpl4_QueryInterface(modern, &IID_IXGameSaveImpl3, (void **)&legacy);
    CHECK(SUCCEEDED(hr) && (void *)legacy == (void *)modern, "legacy interface identity is preserved");
    if (legacy) IXGameSaveImpl3_Release(legacy);
    hr = IXGameSaveImpl4_XGameSaveInitializeProvider(modern, user, scid, FALSE, &provider);
    CHECK(SUCCEEDED(hr) && provider, "classic provider opens through new IID (%#lx)", hr);
    if (provider) {
        hr = IXGameSaveImpl4_XGameSaveGetRemainingQuota(modern, provider, &quota);
        CHECK(SUCCEEDED(hr) && quota > 0, "classic quota uses the correct slot");
        IXGameSaveImpl4_XGameSaveCloseProvider(modern, provider);
    }
    hr = IXGameSaveImpl4_XGameSaveFilesGetFolderWithUiAsync(modern, user, scid, &async);
    CHECK(SUCCEEDED(hr), "legacy Files API starts through new IID (%#lx)",hr);
    hr = IXThreadingImpl_XAsyncGetResultSize(threading,&async,&size);
    CHECK(SUCCEEDED(hr) && size > 1 && size < MAX_PATH,"Files result publishes its actual size");
    if (size > 1 && size < MAX_PATH) {
        folder = malloc(size);
        hr = IXGameSaveImpl4_XGameSaveFilesGetFolderWithUiResult(modern,&async,size,folder);
        CHECK(SUCCEEDED(hr) && strlen(folder)+1 == size,"Files result returns a terminated Windows path");
        if (SUCCEEDED(hr)) {
            snprintf(filename,sizeof(filename),"%s\\interface4-test.dat",folder);
            file=CreateFileA(filename,GENERIC_WRITE|GENERIC_READ,0,NULL,CREATE_ALWAYS,0,NULL);
            CHECK(file != INVALID_HANDLE_VALUE,"returned folder accepts ordinary Win32 file IO");
            if(file != INVALID_HANDLE_VALUE) {
                CHECK(WriteFile(file,"saved",6,&written,NULL) && written==6,"write save content");
                SetFilePointer(file,0,NULL,FILE_BEGIN);
                CHECK(ReadFile(file,readback,6,&read,NULL) && read==6 && !memcmp(readback,"saved",6),"read saved content back");
                CloseHandle(file); DeleteFileA(filename);
            }
        }
        free(folder);
    }
    CHECK(IXGameSaveImpl4_PFXGameSaveInitializeConfig(modern,NULL,&config)==E_NOTIMPL && !config,"cloud config refuses support and clears its output");
    IXGameSaveImpl4_PFXGameSaveFreeConfig(modern,NULL);
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiAsync(modern,NULL,&async)==E_NOTIMPL,"cloud folder does not fake async success");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult(modern,&async,0,NULL)==E_NOTIMPL,"cloud result remains unsupported");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesGetRemainingQuota(modern,NULL,&quota)==E_NOTIMPL,"cloud quota remains unsupported");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetUiCallbacks(modern,NULL)==E_NOTIMPL,"cloud UI callbacks are not falsely registered");
    CHECK(IXGameSaveImpl4_PFXGameSaveProgressUiGetProgress(modern,user,&state,&current,&total)==E_NOTIMPL,"cloud sync is not reported complete");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetProgressUiResponse(modern,user,0)==E_NOTIMPL,"progress response ABI slot");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetSyncFailedUiResponse(modern,user,0)==E_NOTIMPL,"sync response ABI slot");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetActiveDeviceContentionUiResponse(modern,user,0)==E_NOTIMPL,"device contention ABI slot");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetConflictUiResponse(modern,user,0)==E_NOTIMPL,"conflict response ABI slot");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetOutOfStorageUiResponse(modern,user,0)==E_NOTIMPL,"storage response ABI slot");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiAsync(modern,NULL,0,&async)==E_NOTIMPL,"upload does not fake success");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiResult(modern,&async)==E_NOTIMPL,"upload result remains unsupported");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesSetSaveDescriptionAsync(modern,NULL,"test",&async)==E_NOTIMPL,"cloud description remains unsupported");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesSetSaveDescriptionResult(modern,&async)==E_NOTIMPL,"description result ABI slot");
    CHECK(IXGameSaveImpl4_PFXGameSaveSetActiveDeviceChangedCallback(modern,NULL,NULL)==E_NOTIMPL,"device callback ABI slot");
    IXGameSaveImpl4_Release(modern);
}

int main(int argc, char **argv)
{
    HMODULE mod;
    HRESULT (WINAPI *init)( ULONG, ULONG );
    HRESULT (WINAPI *query)( REFCLSID, REFIID, void ** );
    const char *scid = "00000000-0000-0000-0000-000000000001";
    HRESULT hr;

    {
        const char *dll = getenv( "XGR_DLL_PATH" );
        mod = LoadLibraryA( dll && *dll ? dll : "xgameruntime.dll" );
    }
    if (!mod)
    {
        printf( "Bail out! xgameruntime.dll load failed %lu\n", GetLastError() );
        return 100;
    }
    init = (void *)GetProcAddress( mod, "InitializeApiImpl" );
    query = (void *)GetProcAddress( mod, "QueryApiImpl" );
    if (!init || !query) { printf( "Bail out! missing exports\n" ); return 100; }
    g_query = query;
    init( 10002, 7822 );

    if (FAILED( hr = query( &CLSID_XPackageImpl, &IID_IXPackageImpl3, (void **)&pkg ) ) ||
        FAILED( hr = query( &CLSID_XUserImpl, &IID_IXUserImpl, (void **)&users ) ) ||
        FAILED( hr = query( &CLSID_XThreadingImpl, &IID_IXThreadingImpl, (void **)&threading ) ) ||
        FAILED( hr = query( &CLSID_XStoreImpl, &IID_IXStoreImpl6, (void **)&store ) ) ||
        FAILED( hr = query( &CLSID_XGameUiImpl, &IID_IXGameUiImpl4, (void **)&gameui ) ) ||
        FAILED( hr = query( &CLSID_XGameSaveImpl, &IID_IXGameSaveImpl3, (void **)&gs ) ))
    {
        printf( "Bail out! QueryApiImpl failed 0x%08lx\n", hr );
        return 100;
    }

    if (argc > 1 && !strcmp(argv[1], "--store-lifetime"))
    {
        test_store_product_lifetime();
        printf( "%s: %d checks, %d failed\n", g_failed ? "FAILED" : "PASSED", g_run, g_failed );
        return g_failed;
    }
    if (argc > 1 && !strcmp(argv[1], "--manual-queues"))
    {
        test_manual_queues();
        printf( "%s: %d checks, %d failed\n", g_failed ? "FAILED" : "PASSED", g_run, g_failed );
        return g_failed;
    }
    test_xpackage();
    test_xuser();
    test_xstore();
    test_store_product_lifetime();
    test_xgameui();
    test_storage_and_system();
    test_xthreading();
    test_xgamesave( scid );
    test_gamesave_interface4( scid );

    printf( "\n1..%d\n", g_run );
    printf( "%s: %d checks, %d failed\n", g_failed ? "FAILED" : "PASSED", g_run, g_failed );
    return g_failed;
}
