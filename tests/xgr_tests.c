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
#include <stddef.h>
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

#ifndef ARRAY_SIZE
#define ARRAY_SIZE(x) (sizeof(x) / sizeof((x)[0]))
#endif

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
                {
                    BYTE *large = malloc(100000);
                    unsigned int n;
                    for (n = 0; n < 100000; n++) large[n] = n % 251;
                    memset(&token_async, 0, sizeof(token_async));
                    hr = IXUserImpl6_XUserGetTokenAndSignatureUtf16Async(u6, user, 0, L"POST",
                            L"https://fixture.invalid/large", 0, NULL, 100000, large, &token_async);
                    CHECK(hr == S_OK, "100 KB signing request crosses the extended IPC frame intact");
                    hr = IXUserImpl6_XUserGetTokenAndSignatureUtf16Result(u6, &token_async, 512, buffer, &wide, &used);
                    CHECK(hr == S_OK && wide && !wcscmp(wide->signature, L"fixture-signature"),
                          "extended signing request returns the service signature");
                    free(large);
                }
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

    /* a message dialog is dismissed, without a real UI, to its cancel button */
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogAsync( gameui, &async, "Title", "Body",
            "OK", NULL, NULL, XGameUiMessageDialogButton_Second, XGameUiMessageDialogButton_First );
    CHECK( SUCCEEDED( hr ), "ShowMessageDialogAsync succeeds (0x%08lx)", hr );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogResult( gameui, &async, &button );
    CHECK( SUCCEEDED( hr ) && button == XGameUiMessageDialogButton_First,
           "message dialog returns the cancel button (%d)", button );

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


/* ABI fixture results are synthetic; they exercise the facade, not cloud access. */
static LONG pfx_progress_calls;
static void WINAPI pfx_test_progress(XUserHandle u, UINT32 state, void *context)
{
    if (u == user && state == 3 && context == &pfx_progress_calls)
        InterlockedIncrement(&pfx_progress_calls);
}
static void test_pfx_backend(void)
{
    IXGameSaveImpl4 *modern = NULL;
    HMODULE core = LoadLibraryA("PlayFabCore.dll"), save = LoadLibraryA("PlayFabGameSave.dll");
    LONG (WINAPI *closed_users)(void), (*final_uploads)(void);
    struct PFXGameSaveConfigRequest request = { user, "4D5E6F70", "https://fixture.invalid", 0, NULL, NULL, NULL };
    struct PFXGameSaveUiCallbacks ui = { 0 };
    PFXGameSaveConfigHandle config = NULL;
    XAsyncBlock a = { 0 };
    HKEY key;
    DWORD marker = 9, value = 0, bytes = sizeof(value);
    SIZE_T size;
    INT64 quota = -1;
    char folder[128], small[2];
    LONG before;
    UINT32 state;
    UINT64 current, total;
    HRESULT hr;
    printf("# PFX native SDK boundary (synthetic)\n");
    CHECK(sizeof(request) == 56 && offsetof(struct PFXGameSaveConfigRequest, fileLocation) == 48, "configuration matches the public x64 GDK ABI");
    CHECK(sizeof(struct PFXGameSaveDescriptor) == 64 && offsetof(struct PFXGameSaveDescriptor, time) == 40, "descriptor matches the public x64 GDK ABI");
    CHECK(sizeof(ui) == 112, "UI callback table matches the public x64 GDK ABI");
    CHECK(core && save, "native ABI fixtures load");
    if (!core || !save) return;
    {
        HRESULT (WINAPI *register_runtime)(void) = (void *)GetProcAddress(GetModuleHandleA("xgameruntime.dll"), "DllRegisterServer");
        HKEY discovery;
        CHECK(register_runtime && register_runtime() == S_OK, "matching native save SDK registers the PFX facade");
        hr = RegOpenKeyExA(HKEY_LOCAL_MACHINE,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OneSettings\\xbox\\GamingRuntimeServices\\Settings",
                0, KEY_READ, &discovery);
        CHECK(hr == ERROR_SUCCESS, "native SDK can discover the available PFX facade");
        if (hr == ERROR_SUCCESS) RegCloseKey(discovery);
    }
    closed_users = (void *)GetProcAddress(core, "PFFixtureClosedUsers");
    final_uploads = (void *)GetProcAddress(core, "PFFixtureFinalUploads");
    hr = g_query(&CLSID_XGameSaveImpl, &IID_IXGameSaveImpl4, (void **)&modern);
    CHECK(hr == S_OK && modern, "PFX interface is available");
    if (!modern) return;
    RegCreateKeyExA(HKEY_LOCAL_MACHINE, "Software\\Microsoft\\GamingServices", 0, NULL, 0,
                    KEY_ALL_ACCESS, NULL, &key, NULL);
    RegSetValueExA(key, "ForceUseInprocGameSaves", 0, REG_DWORD, (BYTE *)&marker, sizeof(marker));
    hr = IXGameSaveImpl4_PFXGameSaveInitializeConfig(modern, &request, &config);
    CHECK(hr == S_OK && config, "valid configuration initializes the native backend (0x%08lx)", hr);
    RegQueryValueExA(key, "ForceUseInprocGameSaves", NULL, NULL, (BYTE *)&value, &bytes);
    CHECK(value == marker, "temporary provider override restores the pre-existing setting");
    RegCloseKey(key);
    if (!config) { IXGameSaveImpl4_Release(modern); return; }
    ui.progressCallback = pfx_test_progress; ui.progressContext = &pfx_progress_calls;
    hr = IXGameSaveImpl4_PFXGameSaveSetUiCallbacks(modern, &ui);
    memset(&ui, 0, sizeof(ui));
    CHECK(hr == S_OK, "callback registration copies caller-owned storage");
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiAsync(modern, config, &a);
    CHECK(hr == S_OK, "native folder synchronization starts");
    CHECK(IXThreadingImpl_XAsyncGetStatus(threading, &a, FALSE) == E_PENDING, "folder remains pending until native completion");
    CHECK(IXThreadingImpl_XAsyncGetStatus(threading, &a, TRUE) == S_OK, "folder synchronization completes");
    CHECK(pfx_progress_calls == 1, "native progress routes the correct Xbox user and context");
    size = 0;
    hr = IXThreadingImpl_XAsyncGetResultSize(threading, &a, &size);
    CHECK(hr == S_OK && size == sizeof("C:\\pfx-fixture\\account-save"), "folder size is the native result size");
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult(modern, &a, sizeof(small), small);
    CHECK(hr == HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER), "small folder buffer retains the result for retry");
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult(modern, &a, sizeof(folder), folder);
    CHECK(hr == S_OK && !strcmp(folder, "C:\\pfx-fixture\\account-save"), "folder retry returns the native path");
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetRemainingQuota(modern, config, &quota);
    CHECK(hr == S_OK && quota == 987654321, "quota comes from the SDK");
    hr = IXGameSaveImpl4_PFXGameSaveProgressUiGetProgress(modern, user, &state, &current, &total);
    CHECK(hr == S_OK && state == 3 && current == 12 && total == 42, "native progress counters are preserved");
    SetEnvironmentVariableA("PFX_FIXTURE_RESULT", "89237007");
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetRemainingQuota(modern, config, &quota);
    CHECK(hr == (HRESULT)0x89237007, "quota preserves the backend error");
    memset(&a, 0, sizeof(a));
    hr = IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiAsync(modern, config, 0, &a);
    CHECK(hr == S_OK && IXThreadingImpl_XAsyncGetStatus(threading, &a, TRUE) == (HRESULT)0x89237007,
          "upload completes with the real backend failure");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiResult(modern, &a) == (HRESULT)0x89237007,
          "upload result preserves the backend failure");
    SetEnvironmentVariableA("PFX_FIXTURE_RESULT", NULL);
    SetEnvironmentVariableA("PFX_FIXTURE_OFFLINE", "1");
    memset(&a, 0, sizeof(a));
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiAsync(modern, config, &a);
    CHECK(hr == S_OK && IXThreadingImpl_XAsyncGetStatus(threading, &a, TRUE) == (HRESULT)0x80830004,
          "offline synchronization reports the GRTS offline status");
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult(modern, &a, sizeof(folder), folder);
    CHECK(hr == (HRESULT)0x80830004 && !strcmp(folder, "C:\\pfx-fixture\\account-save"),
          "offline result includes usable local folder without claiming cloud success");
    SetEnvironmentVariableA("PFX_FIXTURE_OFFLINE", NULL);
    memset(&a, 0, sizeof(a));
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiAsync(modern, config, &a);
    CHECK(hr == S_OK && IXThreadingImpl_XAsyncGetStatus(threading, &a, TRUE) == S_OK,
          "user reconnects before final cloud synchronization");
    IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult(modern, &a, sizeof(folder), folder);
    before = closed_users();
    memset(&a, 0, sizeof(a));
    hr = IXGameSaveImpl4_PFXGameSaveFilesSetSaveDescriptionAsync(modern, config, "test", &a);
    CHECK(hr == S_OK, "description operation starts");
    IXGameSaveImpl4_PFXGameSaveFreeConfig(modern, config);
    IXGameSaveImpl4_PFXGameSaveFreeConfig(modern, config);
    CHECK(closed_users() == before, "pending operation retains the native user after repeated close");
    CHECK(IXThreadingImpl_XAsyncGetStatus(threading, &a, TRUE) == S_OK &&
          IXGameSaveImpl4_PFXGameSaveFilesSetSaveDescriptionResult(modern, &a) == S_OK,
          "pending description survives config close");
    { unsigned int attempts; for (attempts = 0; attempts < 200 && closed_users() == before; attempts++) Sleep(10); }
    CHECK(final_uploads() == 1, "config close requests one final upload releasing the active device");
    CHECK(closed_users() == before + 1, "final operation releases the native user exactly once");
    CHECK(IXGameSaveImpl4_PFXGameSaveFilesGetRemainingQuota(modern, config, &quota) == E_INVALIDARG,
          "closed config cannot access the backend");
    IXGameSaveImpl4_Release(modern);
}

/* Slots of IXGameSaveImpl4 as numbered in the Microsoft.GDK.PC 2510.3.6286
 * XGameSave and PFXGameSave headers, written out by hand so that a method
 * added, dropped or reordered in xgamesave.idl fails here instead of calling
 * the wrong function in a title. */
static const struct
{
    const char *name;
    size_t offset;
    unsigned int slot;
} gamesave4_slots[] =
{
    { "XGameSaveInitializeProvider", offsetof( IXGameSaveImpl4Vtbl, XGameSaveInitializeProvider ), 3 },
    { "XGameSaveSubmitUpdateResult", offsetof( IXGameSaveImpl4Vtbl, XGameSaveSubmitUpdateResult ), 29 },
    { "XGameSaveFilesGetFolderWithUiAsync", offsetof( IXGameSaveImpl4Vtbl, XGameSaveFilesGetFolderWithUiAsync ), 30 },
    { "XGameSaveFilesGetFolderWithUiResult", offsetof( IXGameSaveImpl4Vtbl, XGameSaveFilesGetFolderWithUiResult ), 31 },
    { "XGameSaveFilesGetRemainingQuota", offsetof( IXGameSaveImpl4Vtbl, XGameSaveFilesGetRemainingQuota ), 32 },
    { "PFXGameSaveInitializeConfig", offsetof( IXGameSaveImpl4Vtbl, PFXGameSaveInitializeConfig ), 33 },
    { "PFXGameSaveSetActiveDeviceChangedCallback", offsetof( IXGameSaveImpl4Vtbl, PFXGameSaveSetActiveDeviceChangedCallback ), 49 },
};

static void test_gamesave_interface4( const char *scid )
{
    static const char payload[] = "saved through IXGameSaveImpl4";
    XGameSaveProviderHandle provider = NULL;
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle update = NULL;
    PFXGameSaveConfigHandle config = (PFXGameSaveConfigHandle)0x1234;
    IXGameSaveImpl4 *modern = NULL, *again = NULL;
    IXGameSaveImpl3 *legacy = NULL;
    IUnknown *unknown = NULL;
    XAsyncBlock async = { 0 };
    XGameSaveBlob *blobs;
    UINT32 count, state = 0;
    UINT64 current = 0, total = 0;
    INT64 quota = -1;
    SIZE_T size = 0;
    char *folder;
    unsigned int i;
    HRESULT hr;

    printf( "# XGameSave through IXGameSaveImpl4\n" );

    hr = g_query( &CLSID_XGameSaveImpl, &IID_IXGameSaveImpl4, (void **)&modern );
    CHECK( SUCCEEDED(hr) && modern, "IXGameSaveImpl4 is available (0x%08lx)", hr );
    if (!modern) return;

    CHECK( sizeof(IXGameSaveImpl4Vtbl) == 50 * sizeof(void *), "the vtable has fifty slots (%Iu)",
           sizeof(IXGameSaveImpl4Vtbl) / sizeof(void *) );
    for (i = 0; i < ARRAY_SIZE(gamesave4_slots); i++)
        CHECK( gamesave4_slots[i].offset == gamesave4_slots[i].slot * sizeof(void *), "%s is slot %u (%Iu)",
               gamesave4_slots[i].name, gamesave4_slots[i].slot, gamesave4_slots[i].offset / sizeof(void *) );

    hr = IXGameSaveImpl4_QueryInterface( modern, &IID_IXGameSaveImpl3, (void **)&legacy );
    CHECK( SUCCEEDED(hr) && (void *)legacy == (void *)modern, "IXGameSaveImpl3 is the same object" );
    if (legacy)
    {
        hr = IXGameSaveImpl3_QueryInterface( legacy, &IID_IXGameSaveImpl4, (void **)&again );
        CHECK( SUCCEEDED(hr) && again == modern, "IXGameSaveImpl3 answers IXGameSaveImpl4 with the same object" );
        if (again) IXGameSaveImpl4_Release( again );
        IXGameSaveImpl3_Release( legacy );
    }
    hr = IXGameSaveImpl4_QueryInterface( modern, &IID_IUnknown, (void **)&unknown );
    CHECK( SUCCEEDED(hr) && (void *)unknown == (void *)modern, "IUnknown is the same object" );
    if (unknown)
    {
        again = NULL;
        hr = IUnknown_QueryInterface( unknown, &IID_IXGameSaveImpl4, (void **)&again );
        CHECK( SUCCEEDED(hr) && again == modern, "IUnknown answers IXGameSaveImpl4 with the same object" );
        if (again) IXGameSaveImpl4_Release( again );
        IUnknown_Release( unknown );
    }

    /* A legacy round trip, every call through the new interface. */
    hr = IXGameSaveImpl4_XGameSaveInitializeProvider( modern, user, scid, FALSE, &provider );
    CHECK( SUCCEEDED(hr) && provider, "InitializeProvider succeeds through IXGameSaveImpl4 (0x%08lx)", hr );
    if (provider)
    {
        hr = IXGameSaveImpl4_XGameSaveGetRemainingQuota( modern, provider, &quota );
        CHECK( SUCCEEDED(hr) && quota > 0, "GetRemainingQuota reaches its slot (0x%08lx)", hr );
        hr = IXGameSaveImpl4_XGameSaveCreateContainer( modern, provider, "interface4_container", &container );
        CHECK( SUCCEEDED(hr) && container, "CreateContainer succeeds (0x%08lx)", hr );
    }
    if (container)
    {
        hr = IXGameSaveImpl4_XGameSaveCreateUpdate( modern, container, "Interface 4", &update );
        CHECK( SUCCEEDED(hr), "CreateUpdate succeeds (0x%08lx)", hr );
        hr = IXGameSaveImpl4_XGameSaveSubmitBlobWrite( modern, update, BLOB_NAME, (UINT8 *)payload, sizeof(payload) );
        CHECK( SUCCEEDED(hr), "SubmitBlobWrite succeeds (0x%08lx)", hr );
        hr = IXGameSaveImpl4_XGameSaveSubmitUpdate( modern, update );
        CHECK( SUCCEEDED(hr), "SubmitUpdate succeeds (0x%08lx)", hr );
        IXGameSaveImpl4_XGameSaveCloseUpdate( modern, update );

        size = sizeof(XGameSaveBlob) + sizeof(payload) + 64;
        blobs = malloc( size );
        CHECK( blobs != NULL, "allocated the read buffer" );
        if (blobs)
        {
            count = 1;
            hr = IXGameSaveImpl4_XGameSaveReadBlobData( modern, container, NULL, &count, size, blobs );
            CHECK( SUCCEEDED(hr) && count == 1 && blobs[0].info.size == sizeof(payload) &&
                   !memcmp( blobs[0].data, payload, sizeof(payload) ),
                   "ReadBlobData returns what was written (0x%08lx, %u)", hr, count );
            free( blobs );
        }
        IXGameSaveImpl4_XGameSaveCloseContainer( modern, container );
        hr = IXGameSaveImpl4_XGameSaveDeleteContainer( modern, provider, "interface4_container" );
        CHECK( SUCCEEDED(hr), "DeleteContainer succeeds (0x%08lx)", hr );
    }
    if (provider) IXGameSaveImpl4_XGameSaveCloseProvider( modern, provider );

    /* XGameSaveFiles: slots 30 and 31, with a real wait before the size. */
    hr = IXGameSaveImpl4_XGameSaveFilesGetFolderWithUiAsync( modern, user, scid, &async );
    CHECK( SUCCEEDED(hr), "FilesGetFolderWithUiAsync starts (0x%08lx)", hr );
    hr = IXThreadingImpl_XAsyncGetStatus( threading, &async, TRUE );
    CHECK( SUCCEEDED(hr), "FilesGetFolderWithUiAsync completes (0x%08lx)", hr );
    size = 0;
    hr = IXThreadingImpl_XAsyncGetResultSize( threading, &async, &size );
    CHECK( SUCCEEDED(hr) && size > 1 && size < MAX_PATH, "the folder result has a plausible size (%Iu)", size );
    if (SUCCEEDED(hr) && size > 1 && size < MAX_PATH)
    {
        folder = malloc( size );
        CHECK( folder != NULL, "allocated the folder buffer" );
        if (folder)
        {
            hr = IXGameSaveImpl4_XGameSaveFilesGetFolderWithUiResult( modern, &async, size, folder );
            CHECK( SUCCEEDED(hr) && strlen( folder ) + 1 == size, "the folder is a terminated path (0x%08lx)", hr );
            free( folder );
        }
    }

    /* Invalid PFX calls must fail before loading a backend. */
    memset( &async, 0, sizeof(async) );
    hr = IXGameSaveImpl4_PFXGameSaveInitializeConfig( modern, NULL, &config );
    CHECK( hr == E_INVALIDARG && !config, "PFXGameSaveInitializeConfig fails and clears its output (0x%08lx)", hr );
    IXGameSaveImpl4_PFXGameSaveFreeConfig( modern, NULL );
    {
        const struct { const char *name; HRESULT hr; HRESULT expected; } cloud[] =
        {
            { "PFXGameSaveFilesGetFolderWithUiAsync", IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiAsync( modern, NULL, &async ), E_INVALIDARG },
            { "PFXGameSaveFilesGetFolderWithUiResult", IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult( modern, &async, 0, NULL ), E_INVALIDARG },
            { "PFXGameSaveFilesGetRemainingQuota", IXGameSaveImpl4_PFXGameSaveFilesGetRemainingQuota( modern, NULL, &quota ), E_INVALIDARG },
            { "PFXGameSaveSetUiCallbacks", IXGameSaveImpl4_PFXGameSaveSetUiCallbacks( modern, NULL ), E_INVALIDARG },
            { "PFXGameSaveProgressUiGetProgress", IXGameSaveImpl4_PFXGameSaveProgressUiGetProgress( modern, user, &state, &current, &total ), E_INVALIDARG },
            { "PFXGameSaveSetProgressUiResponse", IXGameSaveImpl4_PFXGameSaveSetProgressUiResponse( modern, user, 0 ), E_INVALIDARG },
            { "PFXGameSaveSetSyncFailedUiResponse", IXGameSaveImpl4_PFXGameSaveSetSyncFailedUiResponse( modern, user, 0 ), E_INVALIDARG },
            { "PFXGameSaveSetActiveDeviceContentionUiResponse", IXGameSaveImpl4_PFXGameSaveSetActiveDeviceContentionUiResponse( modern, user, 0 ), E_INVALIDARG },
            { "PFXGameSaveSetConflictUiResponse", IXGameSaveImpl4_PFXGameSaveSetConflictUiResponse( modern, user, 0 ), E_INVALIDARG },
            { "PFXGameSaveSetOutOfStorageUiResponse", IXGameSaveImpl4_PFXGameSaveSetOutOfStorageUiResponse( modern, user, 0 ), E_INVALIDARG },
            { "PFXGameSaveFilesUploadWithUiAsync", IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiAsync( modern, NULL, 0, &async ), E_INVALIDARG },
            { "PFXGameSaveFilesUploadWithUiResult", IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiResult( modern, &async ), E_PENDING },
            { "PFXGameSaveFilesSetSaveDescriptionAsync", IXGameSaveImpl4_PFXGameSaveFilesSetSaveDescriptionAsync( modern, NULL, "test", &async ), E_INVALIDARG },
            { "PFXGameSaveFilesSetSaveDescriptionResult", IXGameSaveImpl4_PFXGameSaveFilesSetSaveDescriptionResult( modern, &async ), E_PENDING },
            { "PFXGameSaveSetActiveDeviceChangedCallback", IXGameSaveImpl4_PFXGameSaveSetActiveDeviceChangedCallback( modern, NULL, NULL ), S_OK },
        };

        for (i = 0; i < ARRAY_SIZE(cloud); i++)
            CHECK( cloud[i].hr == cloud[i].expected, "%s validates its arguments/status (0x%08lx)", cloud[i].name, cloud[i].hr );
    }

    IXGameSaveImpl4_Release( modern );
}

static void test_runtime_discovery( HMODULE mod )
{
    HRESULT (WINAPI *register_runtime)(void) = (void *)GetProcAddress( mod, "DllRegisterServer" );
    const UINT64 expected = ((UINT64)35 << 48) | ((UINT64)112 << 32) | ((UINT64)20003 << 16);
    static const WCHAR key[] = L"Software\\Microsoft\\GamingServices";
    UINT64 current = 0, available = 0;
    DWORD size, type;
    LONG status;

    printf( "# Gaming Services runtime discovery\n" );
    CHECK( register_runtime != NULL, "runtime discovery can be registered before initialization" );
    if (!register_runtime) return;
    CHECK( register_runtime() == S_OK, "registering the emulated runtime succeeds" );
    {
        HKEY discovery;
        LONG result = RegOpenKeyExA(HKEY_LOCAL_MACHINE,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OneSettings\\xbox\\GamingRuntimeServices\\Settings",
                0, KEY_READ, &discovery);
        CHECK(result == ERROR_FILE_NOT_FOUND, "legacy registration does not advertise a missing native save backend");
        if (!result) RegCloseKey(discovery);
    }
    size = sizeof(current);
    status = RegGetValueW( HKEY_LOCAL_MACHINE, key, L"CurrentVersion", RRF_RT_REG_QWORD,
                          &type, &current, &size );
    CHECK( status == ERROR_SUCCESS && type == REG_QWORD && size == sizeof(current) && current == expected,
           "current runtime version has the SDK's package-version QWORD layout" );
    size = sizeof(available);
    status = RegGetValueW( HKEY_LOCAL_MACHINE, key, L"AvailableVersion", RRF_RT_REG_QWORD,
                          &type, &available, &size );
    CHECK( status == ERROR_SUCCESS && type == REG_QWORD && available == current,
           "available runtime version matches the installed implementation" );
}

static void test_xgameui_dismisses_to_cancel(void);
static int identity_race_child(void);
static int read_owner_child( const char *scid );
static int pfx_recovery_child(void);
static void test_package_identity_race(void);
static void test_gamesave_unreadable_manifest(void);
static void test_gamesave_user_folder_choice(void);
static void test_gamesave_async_writes(void);
static void test_gamesave_sync_after_async(void);
static void test_gamesave_async_from_callback(void);
static void test_gamesave_queued_at_exit(void);
static int exit_after_async_save_child( const char *container_name );
static void test_pfx_backend_recovery(void);
static void test_pfx_reused_block(void);

int main(int argc, char **argv)
{
    HMODULE mod;
    HRESULT (WINAPI *init)( UINT64, UINT64 );
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
    /* a test's helper process starts from a clean slate and says nothing */
    if (argc < 2 || (strcmp( argv[1], "--identity-race" ) && strcmp( argv[1], "--read-owner" ) &&
                     strcmp( argv[1], "--pfx-recovery-child" ) && strcmp( argv[1], "--exit-after-async-save" )))
        test_runtime_discovery( mod );
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

    if (argc > 1 && !strcmp(argv[1], "--identity-race")) return identity_race_child();
    if (argc > 2 && !strcmp(argv[1], "--read-owner")) return read_owner_child( argv[2] );
    if (argc > 1 && !strcmp(argv[1], "--pfx-recovery-child")) return pfx_recovery_child();
    if (argc > 2 && !strcmp(argv[1], "--exit-after-async-save")) return exit_after_async_save_child( argv[2] );
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
    if (argc > 1 && !strcmp(argv[1], "--pfx"))
    {
        test_xuser();
        test_pfx_backend_recovery();
        test_pfx_backend();
        test_pfx_reused_block();
        printf("%s: %d checks, %d failed\n", g_failed ? "FAILED" : "PASSED", g_run, g_failed);
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
    test_xgameui_dismisses_to_cancel();
    test_package_identity_race();
    test_gamesave_unreadable_manifest();
    test_gamesave_user_folder_choice();
    test_gamesave_async_writes();
    test_gamesave_sync_after_async();
    test_gamesave_async_from_callback();
    test_gamesave_queued_at_exit();

    printf( "\n1..%d\n", g_run );
    printf( "%s: %d checks, %d failed\n", g_failed ? "FAILED" : "PASSED", g_run, g_failed );
    return g_failed;
}

/* ---- XGameUI: a dismissed message dialog -------------------------------- */

static void test_xgameui_dismisses_to_cancel(void)
{
    XGameUiMessageDialogButton button = 99;
    XAsyncBlock async;
    HRESULT hr;

    printf( "# XGameUI message dialog dismissal\n" );

    /* "Delete this save?" with the destructive answer focused: nobody saw it,
     * so nobody chose to delete anything */
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogAsync( gameui, &async, "Corrupted save",
            "Delete it and start over?", "Delete", "Cancel", NULL,
            XGameUiMessageDialogButton_First, XGameUiMessageDialogButton_Second );
    CHECK( SUCCEEDED( hr ), "two-button dialog starts (0x%08lx)", hr );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogResult( gameui, &async, &button );
    CHECK( SUCCEEDED( hr ) && button == XGameUiMessageDialogButton_Second,
           "a dismissed dialog answers with its cancel button, not the focused one (%d)", button );

    /* a cancel button that does not exist cannot be the answer */
    button = 99;
    memset( &async, 0, sizeof(async) );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogAsync( gameui, &async, "Notice", "Body",
            "Continue", "Quit", NULL, XGameUiMessageDialogButton_First, XGameUiMessageDialogButton_Third );
    hr = IXGameUiImpl4_XGameUiShowMessageDialogResult( gameui, &async, &button );
    CHECK( SUCCEEDED( hr ) && button == XGameUiMessageDialogButton_First,
           "a cancel button without text falls back to one that has it (%d)", button );
}

/* ---- helpers for tests that need a process of their own ----------------- */

/* Run this executable again with `args`, XGR_XUID set to `xuid` (or unset),
 * and return its exit code. The package identity and the local XUID are
 * worked out once per process, so a fresh one is the only clean slate. */
static DWORD run_child( const char *args, const char *xuid )
{
    char exe[MAX_PATH], cmd[MAX_PATH + 256], saved[64];
    STARTUPINFOA si = { sizeof(si) };
    PROCESS_INFORMATION pi;
    DWORD code = 255, had;

    if (!GetModuleFileNameA( NULL, exe, sizeof(exe) )) return code;
    snprintf( cmd, sizeof(cmd), "\"%s\" %s", exe, args );
    had = GetEnvironmentVariableA( "XGR_XUID", saved, sizeof(saved) );
    SetEnvironmentVariableA( "XGR_XUID", xuid );
    fflush( stdout );
    if (CreateProcessA( exe, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi ))
    {
        if (WaitForSingleObject( pi.hProcess, 60000 ) == WAIT_OBJECT_0) GetExitCodeProcess( pi.hProcess, &code );
        else TerminateProcess( pi.hProcess, 254 );
        CloseHandle( pi.hThread );
        CloseHandle( pi.hProcess );
    }
    SetEnvironmentVariableA( "XGR_XUID", had && had < sizeof(saved) ? saved : NULL );
    return code;
}

static XUserHandle add_quiet_user(void)
{
    XAsyncBlock async = { 0 };
    XUserHandle added = NULL;

    if (FAILED( IXUserImpl_XUserAddAsync( users, XUserAddOptions_AddDefaultUserSilently, &async ) )) return NULL;
    if (FAILED( IXUserImpl_XUserAddResult( users, &async, &added ) )) return NULL;
    return added;
}

/* The save root the harness points XGR_WGS_ROOT at, as a Windows path. */
static BOOL save_root( char *out, DWORD size )
{
    DWORD len = GetEnvironmentVariableA( "XGR_WGS_ROOT", out, size );
    return len && len < size;
}

/* The one entry of `dir` matching `pattern`, of the kind asked for. */
static BOOL find_entry( const char *dir, const char *pattern, BOOL want_dir, char *out, size_t size )
{
    WIN32_FIND_DATAA data;
    char spec[MAX_PATH];
    HANDLE find;
    BOOL found = FALSE;

    snprintf( spec, sizeof(spec), "%s\\%s", dir, pattern );
    if ((find = FindFirstFileA( spec, &data )) == INVALID_HANDLE_VALUE) return FALSE;
    do
    {
        if (!strcmp( data.cFileName, "." ) || !strcmp( data.cFileName, ".." )) continue;
        if (!!(data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != !!want_dir) continue;
        snprintf( out, size, "%s\\%s", dir, data.cFileName );
        found = TRUE;
        break;
    } while (FindNextFileA( find, &data ));
    FindClose( find );
    return found;
}

static HRESULT write_one_blob( XGameSaveProviderHandle provider, const char *container_name,
                               const char *blob, const char *value )
{
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle update = NULL;
    HRESULT hr;

    if (FAILED( hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, container_name, &container ) )) return hr;
    if (SUCCEEDED( hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, container_name, &update ) ))
    {
        hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, blob, (UINT8 *)value, strlen( value ) + 1 );
        if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitUpdate( gs, update );
        IXGameSaveImpl3_XGameSaveCloseUpdate( gs, update );
    }
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    return hr;
}

/* What blob `blob` of `container_name` holds, or "" if it cannot be read. */
static HRESULT read_one_blob( XGameSaveProviderHandle provider, const char *container_name,
                              const char *blob, char *out, size_t size )
{
    XGameSaveContainerHandle container = NULL;
    const char *names[1] = { blob };
    union { XGameSaveBlob blob; char bytes[512]; } buffer;
    UINT32 count = 1;
    HRESULT hr;

    out[0] = 0;
    if (FAILED( hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, container_name, &container ) )) return hr;
    hr = IXGameSaveImpl3_XGameSaveReadBlobData( gs, container, names, &count, sizeof(buffer), &buffer.blob );
    if (SUCCEEDED( hr ) && count == 1 && buffer.blob.info.size && buffer.blob.info.size <= size)
    {
        memcpy( out, buffer.blob.data, buffer.blob.info.size );
        out[buffer.blob.info.size - 1] = 0;
    }
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    return hr;
}

/* ---- package identity, resolved once from many threads ------------------ */

static volatile LONG race_go, race_failures;
static IXPersistentLocalStorageImpl3 *race_pls;

static DWORD WINAPI identity_race_thread( void *arg )
{
    SIZE_T size = 0;

    while (!race_go) YieldProcessor();
    if (FAILED( IXPersistentLocalStorageImpl3_XPersistentLocalStorageGetPathSize( race_pls, &size ) ) || size <= 1)
        InterlockedIncrement( &race_failures );
    return 0;
}

/* Child: sixteen threads ask for the package's storage at the same instant,
 * the first time anything in the process needs the package identity. */
static int identity_race_child(void)
{
    HANDLE threads[16];
    unsigned int i, started = 0;

    if (FAILED( g_query( &CLSID_XPersistentLocalStorageImpl, &IID_IXPersistentLocalStorageImpl3, (void **)&race_pls ) ))
        return 100;
    for (i = 0; i < ARRAY_SIZE(threads); i++)
        if ((threads[started] = CreateThread( NULL, 0, identity_race_thread, NULL, 0, NULL ))) started++;
    Sleep( 100 );
    race_go = 1;
    WaitForMultipleObjects( started, threads, TRUE, 30000 );
    for (i = 0; i < started; i++) CloseHandle( threads[i] );
    return race_failures;
}

static void test_package_identity_race(void)
{
    DWORD code;

    printf( "# package identity from concurrent first callers\n" );
    code = run_child( "--identity-race", NULL );
    CHECK( code == 0, "no first caller finds the package identity missing (%lu failed)", code );
}

/* ---- XGameSave: an unreadable container manifest ------------------------ */

static void test_gamesave_unreadable_manifest(void)
{
    static const char scid[] = "00000000-0000-0000-0000-0000000000e3";
    XGameSaveProviderHandle provider = NULL;
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle update = NULL;
    char root[MAX_PATH], dir[MAX_PATH], folder[MAX_PATH], manifest[MAX_PATH], value[64];
    HANDLE locked;
    HRESULT hr;

    printf( "# XGameSave with an unreadable container manifest\n" );
    hr = IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, scid, FALSE, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "provider opens (0x%08lx)", hr );
    if (!provider) return;

    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, "manifest_test", &container );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, "Manifest", &update );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, "progress", (UINT8 *)"chapter 9", 10 );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, "settings", (UINT8 *)"old", 4 );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitUpdate( gs, update );
    if (update) IXGameSaveImpl3_XGameSaveCloseUpdate( gs, update );
    if (container) IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    CHECK( SUCCEEDED( hr ), "a container with two blobs is written (0x%08lx)", hr );

    if (!save_root( root, sizeof(root) ) || !find_entry( root, "*_000000000000000000000000000000E3", TRUE, dir, sizeof(dir) ) ||
        !find_entry( dir, "*", TRUE, folder, sizeof(folder) ) ||
        !find_entry( folder, "container.*", FALSE, manifest, sizeof(manifest) ))
    {
        CHECK( 0, "found the container manifest on disk" );
        IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
        return;
    }

    /* the manifest is there but cannot be opened: another program has it */
    locked = CreateFileA( manifest, GENERIC_READ, 0, NULL, OPEN_EXISTING, 0, NULL );
    CHECK( locked != INVALID_HANDLE_VALUE, "the manifest is held open without sharing" );
    hr = write_one_blob( provider, "manifest_test", "settings", "new" );
    CHECK( FAILED( hr ), "an update to a container whose manifest cannot be read fails (0x%08lx)", hr );
    CloseHandle( locked );

    hr = read_one_blob( provider, "manifest_test", "progress", value, sizeof(value) );
    CHECK( SUCCEEDED( hr ) && !strcmp( value, "chapter 9" ),
           "the container's other blob survives that update (0x%08lx, '%s')", hr, value );
    hr = write_one_blob( provider, "manifest_test", "settings", "new" );
    CHECK( SUCCEEDED( hr ), "once readable again, the update goes through (0x%08lx)", hr );
    hr = read_one_blob( provider, "manifest_test", "progress", value, sizeof(value) );
    CHECK( SUCCEEDED( hr ) && !strcmp( value, "chapter 9" ), "and keeps the other blob (0x%08lx, '%s')", hr, value );

    IXGameSaveImpl3_XGameSaveDeleteContainer( gs, provider, "manifest_test" );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
}

/* ---- XGameSave: which <XUID>_<SCID> folder is used ---------------------- */

static void set_index_age( const char *dir, BOOL old )
{
    char path[MAX_PATH];
    SYSTEMTIME st = { 2001, 1, 1, 1 };
    FILETIME ft;
    HANDLE file;

    snprintf( path, sizeof(path), "%s\\containers.index", dir );
    if (old) SystemTimeToFileTime( &st, &ft );
    else GetSystemTimeAsFileTime( &ft );
    file = CreateFileA( path, FILE_WRITE_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, 0, NULL );
    if (file == INVALID_HANDLE_VALUE) return;
    SetFileTime( file, NULL, NULL, &ft );
    CloseHandle( file );
}

/* Write a one-blob profile through `scid`, then move its folder to `name`. */
static BOOL make_profile( const char *scid, const char *suffix, const char *who, const char *name, BOOL old )
{
    XGameSaveProviderHandle provider = NULL;
    char root[MAX_PATH], from[MAX_PATH], pattern[64], to[MAX_PATH];
    BOOL ok;

    if (FAILED( IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, scid, FALSE, &provider ) )) return FALSE;
    ok = SUCCEEDED( write_one_blob( provider, "profile", "who", who ) );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
    snprintf( pattern, sizeof(pattern), "*_%s", suffix );
    if (!ok || !save_root( root, sizeof(root) ) || !find_entry( root, pattern, TRUE, from, sizeof(from) )) return FALSE;
    snprintf( to, sizeof(to), "%s\\%s", root, name );
    if (!MoveFileA( from, to )) return FALSE;
    set_index_age( to, old );
    return TRUE;
}

/* Child: whose profile does a provider for `scid` open? */
static int read_owner_child( const char *scid )
{
    XGameSaveProviderHandle provider = NULL;
    char who[64];

    if (!(user = add_quiet_user())) return 100;
    if (FAILED( IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, scid, FALSE, &provider ) )) return 101;
    read_one_blob( provider, "profile", "who", who, sizeof(who) );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
    if (!strcmp( who, "configured" )) return 1;
    if (!strcmp( who, "placeholder" )) return 2;
    if (!strcmp( who, "older" )) return 3;
    if (!strcmp( who, "newer" )) return 4;
    return 5;
}

static void test_gamesave_user_folder_choice(void)
{
    static const char sfx_a[] = "0000000000000000000000000000A001", sfx_b[] = "0000000000000000000000000000B001";
    char name[64];
    BOOL ok;
    DWORD code;

    printf( "# XGameSave folder choice among several users\n" );

    /* the configured user's folder wins, even over a newer placeholder that
     * the directory listing returns first */
    snprintf( name, sizeof(name), "0000000000000000_%s", sfx_a );
    ok = make_profile( "00000000-0000-0000-0000-00000000A0F1", "0000000000000000000000000000A0F1", "placeholder", name, FALSE );
    snprintf( name, sizeof(name), "00000000000ABCDE_%s", sfx_a );
    ok = ok && make_profile( "00000000-0000-0000-0000-00000000A0F2", "0000000000000000000000000000A0F2", "configured", name, TRUE );
    CHECK( ok, "two profiles for one service configuration are set up" );
    code = run_child( "--read-owner 00000000-0000-0000-0000-00000000a001", "00000000000ABCDE" );
    CHECK( code == 1, "XGR_XUID picks that user's folder (child %lu)", code );

    /* with no user configured, the profile written last is the one in use */
    snprintf( name, sizeof(name), "0000000000000001_%s", sfx_b );
    ok = make_profile( "00000000-0000-0000-0000-00000000B0F1", "0000000000000000000000000000B0F1", "older", name, TRUE );
    snprintf( name, sizeof(name), "0000000000000002_%s", sfx_b );
    ok = ok && make_profile( "00000000-0000-0000-0000-00000000B0F2", "0000000000000000000000000000B0F2", "newer", name, FALSE );
    CHECK( ok, "an older and a newer profile are set up" );
    code = run_child( "--read-owner 00000000-0000-0000-0000-00000000b001", NULL );
    CHECK( code == 4, "without XGR_XUID the most recently written profile is used (child %lu)", code );
    code = run_child( "--read-owner 00000000-0000-0000-0000-00000000b001", NULL );
    CHECK( code == 4, "and the same one again on the next run (child %lu)", code );
}

/* ---- XGameSave: asynchronous writes leave the caller's thread ----------- */

static HANDLE hold_inside, hold_release;

static BOOLEAN __stdcall hold_save_lock( const XGameSaveContainerInfo *info, void *context )
{
    SetEvent( hold_inside );
    WaitForSingleObject( hold_release, 10000 );
    return FALSE;
}

static DWORD WINAPI hold_saves_thread( void *provider )
{
    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, NULL, hold_save_lock );
    return 0;
}

struct async_submit
{
    XGameSaveUpdateHandle update;
    XAsyncBlock *async;
    HRESULT hr;
};

static DWORD WINAPI submit_async_thread( void *arg )
{
    struct async_submit *submit = arg;
    submit->hr = IXGameSaveImpl3_XGameSaveSubmitUpdateAsync( gs, submit->update, submit->async );
    return 0;
}

static void test_gamesave_async_writes(void)
{
    static const char scid[] = "00000000-0000-0000-0000-0000000000f5";
    XGameSaveProviderHandle provider = NULL;
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle first = NULL, second = NULL;
    XAsyncBlock a1, a2, a3;
    struct async_submit submit;
    HANDLE holder = NULL, submitter;
    char value[64];
    int blob_count = -1;
    BOOL returned;
    HRESULT hr;

    printf( "# XGameSave asynchronous writes\n" );
    hr = IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, scid, FALSE, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "provider opens (0x%08lx)", hr );
    if (!provider) return;
    hr = write_one_blob( provider, "unit_test_container", "order", "initial" );
    CHECK( SUCCEEDED( hr ), "a container exists to write to (0x%08lx)", hr );

    /* another thread is in the middle of a save call, holding the saves */
    hold_inside = CreateEventA( NULL, TRUE, FALSE, NULL );
    hold_release = CreateEventA( NULL, TRUE, FALSE, NULL );
    holder = CreateThread( NULL, 0, hold_saves_thread, provider, 0, NULL );
    CHECK( WaitForSingleObject( hold_inside, 5000 ) == WAIT_OBJECT_0, "another thread holds the saves" );

    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, "unit_test_container", &container );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, NULL, &first );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, first, "order", (UINT8 *)"first", 6 );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, NULL, &second );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, second, "order", (UINT8 *)"second", 7 );
    CHECK( SUCCEEDED( hr ), "two updates are prepared (0x%08lx)", hr );

    memset( &a1, 0, sizeof(a1) );
    memset( &a2, 0, sizeof(a2) );
    submit.update = first;
    submit.async = &a1;
    submit.hr = E_FAIL;
    submitter = CreateThread( NULL, 0, submit_async_thread, &submit, 0, NULL );
    returned = WaitForSingleObject( submitter, 2000 ) == WAIT_OBJECT_0;
    CHECK( returned && SUCCEEDED( submit.hr ), "SubmitUpdateAsync returns without waiting for the disk (0x%08lx)", submit.hr );
    if (returned)
    {
        CHECK( IXThreadingImpl_XAsyncGetStatus( threading, &a1, FALSE ) == E_PENDING, "the write is still pending" );
        hr = IXGameSaveImpl3_XGameSaveSubmitUpdateAsync( gs, second, &a2 );
        CHECK( SUCCEEDED( hr ), "a second update queues behind it (0x%08lx)", hr );
        /* the title lets go of its handles straight away */
        IXGameSaveImpl3_XGameSaveCloseUpdate( gs, first );
        IXGameSaveImpl3_XGameSaveCloseUpdate( gs, second );
        IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    }
    SetEvent( hold_release );
    WaitForSingleObject( holder, 15000 );
    WaitForSingleObject( submitter, 15000 );
    CloseHandle( holder );
    CloseHandle( submitter );
    CloseHandle( hold_inside );
    CloseHandle( hold_release );
    if (!returned)
    {
        IXGameSaveImpl3_XGameSaveCloseUpdate( gs, first );
        IXGameSaveImpl3_XGameSaveCloseUpdate( gs, second );
        IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    }
    else
    {
        hr = IXThreadingImpl_XAsyncGetStatus( threading, &a1, TRUE );
        CHECK( hr == S_OK && IXGameSaveImpl3_XGameSaveSubmitUpdateResult( gs, &a1 ) == S_OK,
               "the first write completes once the saves are free (0x%08lx)", hr );
        hr = IXThreadingImpl_XAsyncGetStatus( threading, &a2, TRUE );
        CHECK( hr == S_OK && IXGameSaveImpl3_XGameSaveSubmitUpdateResult( gs, &a2 ) == S_OK,
               "the second completes too (0x%08lx)", hr );
    }
    hr = read_one_blob( provider, "unit_test_container", "order", value, sizeof(value) );
    CHECK( SUCCEEDED( hr ) && !strcmp( value, "second" ), "the writes reached the disk in order ('%s')", value );

    memset( &a3, 0, sizeof(a3) );
    hr = IXGameSaveImpl3_XGameSaveDeleteContainerAsync( gs, provider, "unit_test_container", &a3 );
    CHECK( SUCCEEDED( hr ), "DeleteContainerAsync starts (0x%08lx)", hr );
    hr = IXThreadingImpl_XAsyncGetStatus( threading, &a3, TRUE );
    CHECK( hr == S_OK && IXGameSaveImpl3_XGameSaveDeleteContainerResult( gs, &a3 ) == S_OK,
           "the delete completes (0x%08lx)", hr );
    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, &blob_count, find_container );
    CHECK( blob_count == -1, "and the container is gone" );
    memset( &a3, 0, sizeof(a3) );
    hr = IXGameSaveImpl3_XGameSaveDeleteContainerAsync( gs, provider, "bad name!", &a3 );
    CHECK( SUCCEEDED( hr ) && IXThreadingImpl_XAsyncGetStatus( threading, &a3, TRUE ) == E_GS_INVALID_CONTAINER_NAME,
           "an invalid name still completes with its error" );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
}

/* ---- PFX: the cached backend copy and the provider override ------------- */

static BOOL pfx_backend_paths( char *source, char *destination, size_t size, WIN32_FILE_ATTRIBUTE_DATA *attributes )
{
    HMODULE save = LoadLibraryA( "PlayFabGameSave.dll" );
    char local[MAX_PATH], family[128], dir[MAX_PATH];
    char *p;

    if (!save || !GetModuleFileNameA( save, source, size ) ||
        !GetFileAttributesExA( source, GetFileExInfoStandard, attributes ) ||
        !GetEnvironmentVariableA( "LOCALAPPDATA", local, sizeof(local) ) ||
        !GetEnvironmentVariableA( "XGR_EXPECTED_FAMILY", family, sizeof(family) ))
        return FALSE;
    snprintf( dir, sizeof(dir), "%s\\Packages\\%s\\SystemAppData\\PFXGameSave\\backend", local, family );
    for (p = dir + 3; *p; p++) if (*p == '\\') { *p = 0; CreateDirectoryA( dir, NULL ); *p = '\\'; }
    CreateDirectoryA( dir, NULL );
    snprintf( destination, size, "%s\\pgs-%08lx-%08lx-%08lx.dll", dir, attributes->ftLastWriteTime.dwHighDateTime,
              attributes->ftLastWriteTime.dwLowDateTime, attributes->nFileSizeLow );
    return TRUE;
}

/* Child: the backend loads from a cache holding a cut-short copy, and an
 * override an earlier run left behind is put back. */
static int pfx_recovery_child(void)
{
    static const char marker_key[] = "Software\\Wine\\xgameruntime\\ForceUseInprocGameSaves";
    struct PFXGameSaveConfigRequest request = { NULL, "4D5E6F70", "https://fixture.invalid", 0, NULL, NULL, NULL };
    PFXGameSaveConfigHandle config = NULL;
    IXGameSaveImpl4 *modern = NULL;
    DWORD one = 1, value = 0, size = sizeof(value);
    HKEY key, marker;
    LONG status;
    HRESULT hr;

    if (!(request.requestingUser = add_quiet_user())) return 100;
    if (!LoadLibraryA( "PlayFabCore.dll" ) || !LoadLibraryA( "PlayFabGameSave.dll" )) return 101;
    if (FAILED( g_query( &CLSID_XGameSaveImpl, &IID_IXGameSaveImpl4, (void **)&modern ) )) return 102;

    /* what a run killed inside the private SDK's initialization leaves: the
     * override in place, and the note that the setting did not exist before */
    RegCreateKeyExA( HKEY_LOCAL_MACHINE, "Software\\Microsoft\\GamingServices", 0, NULL, 0, KEY_ALL_ACCESS, NULL, &key, NULL );
    RegSetValueExA( key, "ForceUseInprocGameSaves", 0, REG_DWORD, (BYTE *)&one, sizeof(one) );
    if (!RegCreateKeyExA( HKEY_LOCAL_MACHINE, marker_key, 0, NULL, 0, KEY_ALL_ACCESS, NULL, &marker, NULL ))
        RegCloseKey( marker );

    hr = IXGameSaveImpl4_PFXGameSaveInitializeConfig( modern, &request, &config );
    CHECK( hr == S_OK && config, "the backend loads despite a cut-short cached copy (0x%08lx)", hr );
    status = RegQueryValueExA( key, "ForceUseInprocGameSaves", NULL, NULL, (BYTE *)&value, &size );
    CHECK( status == ERROR_FILE_NOT_FOUND, "an override left by an interrupted run is removed (%ld, %lu)", status, value );
    status = RegOpenKeyExA( HKEY_LOCAL_MACHINE, marker_key, 0, KEY_READ, &marker );
    CHECK( status == ERROR_FILE_NOT_FOUND, "and nothing is left marked as pending (%ld)", status );
    if (!status) RegCloseKey( marker );
    RegCloseKey( key );
    if (config) IXGameSaveImpl4_PFXGameSaveFreeConfig( modern, config );
    IXGameSaveImpl4_Release( modern );
    return g_failed;
}

static void test_pfx_backend_recovery(void)
{
    char source[MAX_PATH], destination[MAX_PATH];
    WIN32_FILE_ATTRIBUTE_DATA attributes, copy;
    HANDLE file;
    DWORD written = 0, code;

    printf( "# PFX backend copy and provider override recovery\n" );
    if (!pfx_backend_paths( source, destination, sizeof(source), &attributes ))
    {
        CHECK( 0, "found the PlayFab save backend and its cache" );
        return;
    }
    /* a copy cut short by a full disk or a killed process */
    file = CreateFileA( destination, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL );
    if (file != INVALID_HANDLE_VALUE)
    {
        WriteFile( file, "MZ", 2, &written, NULL );
        CloseHandle( file );
    }
    CHECK( written == 2, "a truncated backend copy is in the cache" );
    code = run_child( "--pfx-recovery-child", NULL );
    CHECK( code == 0, "the backend recovers in a fresh process (%lu failed)", code );
    CHECK( GetFileAttributesExA( destination, GetFileExInfoStandard, &copy ) &&
           copy.nFileSizeLow == attributes.nFileSizeLow && copy.nFileSizeHigh == attributes.nFileSizeHigh,
           "the cached copy is whole again" );
}

/* ---- PFX: a reused XAsyncBlock ------------------------------------------ */

static void test_pfx_reused_block(void)
{
    struct PFXGameSaveConfigRequest request = { user, "4D5E6F70", "https://fixture.invalid", 0, NULL, NULL, NULL };
    PFXGameSaveConfigHandle config = NULL;
    IXGameSaveImpl4 *modern = NULL;
    XUserHandle added = NULL;
    XAsyncBlock a = { 0 }, b = { 0 };
    char folder[128], generic[128];
    SIZE_T used = 0;
    HRESULT hr;

    printf( "# PFX operations on a reused XAsyncBlock\n" );
    if (FAILED( g_query( &CLSID_XGameSaveImpl, &IID_IXGameSaveImpl4, (void **)&modern ) ))
    {
        CHECK( 0, "PFX interface is available" );
        return;
    }
    hr = IXGameSaveImpl4_PFXGameSaveInitializeConfig( modern, &request, &config );
    CHECK( hr == S_OK && config, "a configuration initializes (0x%08lx)", hr );
    if (!config)
    {
        IXGameSaveImpl4_Release( modern );
        return;
    }

    /* a block that already carries a finished result */
    hr = IXGameSaveImpl4_PFXGameSaveFilesUploadWithUiAsync( modern, config, 0, &a );
    CHECK( hr == S_OK && IXThreadingImpl_XAsyncGetStatus( threading, &a, TRUE ) == S_OK, "an upload completes on the block" );
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiAsync( modern, config, &a );
    CHECK( hr == S_OK, "the same block starts a folder synchronization" );
    CHECK( IXThreadingImpl_XAsyncGetStatus( threading, &a, FALSE ) == E_PENDING,
           "the reused block is pending, not complete with the upload's result" );
    CHECK( IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult( modern, &a, sizeof(folder), folder ) == E_PENDING,
           "its result is not available yet" );
    CHECK( IXThreadingImpl_XAsyncGetStatus( threading, &a, TRUE ) == S_OK, "the synchronization completes" );
    hr = IXThreadingImpl_XAsyncGetResult( threading, &a, NULL, sizeof(generic), generic, &used );
    CHECK( hr == S_OK && used == sizeof("C:\\pfx-fixture\\account-save") && !strcmp( generic, "C:\\pfx-fixture\\account-save" ),
           "generic retrieval copies the folder path (0x%08lx)", hr );
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult( modern, &a, sizeof(folder), folder );
    CHECK( hr == S_OK && !strcmp( folder, "C:\\pfx-fixture\\account-save" ), "the typed result returns the folder" );

    /* a block that another API completed is not a folder result */
    hr = IXUserImpl_XUserAddAsync( users, XUserAddOptions_AddDefaultUserSilently, &b );
    CHECK( SUCCEEDED( hr ), "another API completes a block" );
    hr = IXGameSaveImpl4_PFXGameSaveFilesGetFolderWithUiResult( modern, &b, sizeof(folder), folder );
    CHECK( hr == E_INVALIDARG, "the folder result refuses a block it did not complete (0x%08lx)", hr );
    if (SUCCEEDED( IXUserImpl_XUserAddResult( users, &b, &added ) ) && added) IXUserImpl_XUserCloseHandle( users, added );

    IXGameSaveImpl4_PFXGameSaveFreeConfig( modern, config );
    IXGameSaveImpl4_Release( modern );
}

/* ---- XGameSave: queued writes keep their order -------------------------- */

struct sync_inside_hold
{
    XGameSaveProviderHandle provider;
    XGameSaveUpdateHandle update;
    HANDLE inside, go, done;
    HRESULT hr;
};

/* Holds the saves like hold_save_lock, then, still inside the callback, makes
 * a synchronous update: the thread already owns the saves, so nothing about
 * lock order decides whether it lands before or after a queued one. */
static BOOLEAN __stdcall sync_submit_while_holding( const XGameSaveContainerInfo *info, void *context )
{
    struct sync_inside_hold *hold = context;

    if (!hold->update) return FALSE;
    SetEvent( hold->inside );
    WaitForSingleObject( hold->go, 10000 );
    hold->hr = IXGameSaveImpl3_XGameSaveSubmitUpdate( gs, hold->update );
    hold->update = NULL;
    SetEvent( hold->done );
    return FALSE;
}

static DWORD WINAPI sync_submit_thread( void *context )
{
    struct sync_inside_hold *hold = context;

    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, hold->provider, hold, sync_submit_while_holding );
    return 0;
}

static void test_gamesave_sync_after_async(void)
{
    static const char scid[] = "00000000-0000-0000-0000-0000000000f6";
    XGameSaveProviderHandle provider = NULL;
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle early = NULL, late = NULL;
    struct sync_inside_hold hold;
    XAsyncBlock async;
    HANDLE thread;
    char value[64];
    HRESULT hr;

    printf( "# XGameSave synchronous update after a queued one\n" );
    hr = IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, scid, FALSE, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "provider opens (0x%08lx)", hr );
    if (!provider) return;
    hr = write_one_blob( provider, "order_container", "order", "initial" );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, "order_container", &container );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, NULL, &early );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, early, "order", (UINT8 *)"async", 6 );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, NULL, &late );
    if (SUCCEEDED( hr )) hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, late, "order", (UINT8 *)"sync", 5 );
    CHECK( SUCCEEDED( hr ), "an async and a later synchronous update are prepared (0x%08lx)", hr );
    if (FAILED( hr )) goto done;

    memset( &hold, 0, sizeof(hold) );
    hold.provider = provider;
    hold.update = late;
    hold.hr = E_FAIL;
    hold.inside = CreateEventA( NULL, TRUE, FALSE, NULL );
    hold.go = CreateEventA( NULL, TRUE, FALSE, NULL );
    hold.done = CreateEventA( NULL, TRUE, FALSE, NULL );
    thread = CreateThread( NULL, 0, sync_submit_thread, &hold, 0, NULL );
    CHECK( WaitForSingleObject( hold.inside, 5000 ) == WAIT_OBJECT_0, "another thread holds the saves" );

    /* the async update is made first, the synchronous one after it */
    memset( &async, 0, sizeof(async) );
    hr = IXGameSaveImpl3_XGameSaveSubmitUpdateAsync( gs, early, &async );
    CHECK( SUCCEEDED( hr ), "the async update is queued (0x%08lx)", hr );
    SetEvent( hold.go );
    CHECK( WaitForSingleObject( hold.done, 10000 ) == WAIT_OBJECT_0 && hold.hr == S_OK,
           "the synchronous update is made after it (0x%08lx)", hold.hr );
    WaitForSingleObject( thread, 15000 );
    CloseHandle( thread );
    CloseHandle( hold.inside );
    CloseHandle( hold.go );
    CloseHandle( hold.done );

    hr = IXThreadingImpl_XAsyncGetStatus( threading, &async, TRUE );
    CHECK( hr == S_OK, "the async update completes (0x%08lx)", hr );
    hr = read_one_blob( provider, "order_container", "order", value, sizeof(value) );
    CHECK( SUCCEEDED( hr ) && !strcmp( value, "sync" ), "the later synchronous update is what is on disk ('%s')", value );

done:
    if (early) IXGameSaveImpl3_XGameSaveCloseUpdate( gs, early );
    if (late) IXGameSaveImpl3_XGameSaveCloseUpdate( gs, late );
    if (container) IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
}

/* ---- XGameSave: an async call from inside an enumeration ---------------- */

struct reentrant_call
{
    XGameSaveProviderHandle provider;
    XGameSaveContainerHandle container;
    XAsyncBlock async;
    HRESULT started, status;
    BOOL called;
};

/* Poll for up to three seconds rather than wait: if the call cannot finish
 * while the callback runs, a wait would never return. */
static HRESULT poll_status( XAsyncBlock *async )
{
    HRESULT hr;
    DWORD start = GetTickCount();

    while ((hr = IXThreadingImpl_XAsyncGetStatus( threading, async, FALSE )) == E_PENDING &&
           GetTickCount() - start < 3000)
        Sleep( 10 );
    return hr;
}

static BOOLEAN __stdcall delete_from_enumeration( const XGameSaveContainerInfo *info, void *context )
{
    struct reentrant_call *call = context;

    if (call->called || strcmp( info->name, "reentrant_delete" )) return TRUE;
    call->called = TRUE;
    memset( &call->async, 0, sizeof(call->async) );
    call->started = IXGameSaveImpl3_XGameSaveDeleteContainerAsync( gs, call->provider, info->name, &call->async );
    call->status = poll_status( &call->async );
    return FALSE;
}

static BOOLEAN __stdcall submit_from_enumeration( const XGameSaveBlobInfo *info, void *context )
{
    struct reentrant_call *call = context;
    XGameSaveUpdateHandle update = NULL;

    if (call->called) return FALSE;
    call->called = TRUE;
    call->started = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, call->container, NULL, &update );
    if (SUCCEEDED( call->started ))
        call->started = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, "inner", (UINT8 *)"written", 8 );
    memset( &call->async, 0, sizeof(call->async) );
    if (SUCCEEDED( call->started )) call->started = IXGameSaveImpl3_XGameSaveSubmitUpdateAsync( gs, update, &call->async );
    if (update) IXGameSaveImpl3_XGameSaveCloseUpdate( gs, update );
    call->status = SUCCEEDED( call->started ) ? poll_status( &call->async ) : call->started;
    return FALSE;
}

static BOOLEAN __stdcall find_reentrant_delete( const XGameSaveContainerInfo *info, void *context )
{
    if (!strcmp( info->name, "reentrant_delete" )) *(BOOL *)context = TRUE;
    return TRUE;
}

static void test_gamesave_async_from_callback(void)
{
    static const char scid[] = "00000000-0000-0000-0000-0000000000f7";
    XGameSaveProviderHandle provider = NULL;
    struct reentrant_call call;
    char value[64];
    BOOL found = FALSE;
    HRESULT hr;

    printf( "# XGameSave async calls from inside an enumeration callback\n" );
    hr = IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, scid, FALSE, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "provider opens (0x%08lx)", hr );
    if (!provider) return;
    hr = write_one_blob( provider, "reentrant_delete", "blob", "doomed" );
    if (SUCCEEDED( hr )) hr = write_one_blob( provider, "reentrant_submit", "outer", "present" );
    CHECK( SUCCEEDED( hr ), "two containers exist (0x%08lx)", hr );

    memset( &call, 0, sizeof(call) );
    call.provider = provider;
    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, &call, delete_from_enumeration );
    CHECK( call.called && SUCCEEDED( call.started ), "DeleteContainerAsync starts inside the callback (0x%08lx)", call.started );
    CHECK( call.status == S_OK, "and completes while the callback still runs (0x%08lx)", call.status );
    if (call.status == E_PENDING) IXThreadingImpl_XAsyncGetStatus( threading, &call.async, TRUE );
    IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, &found, find_reentrant_delete );
    CHECK( !found, "the container is gone" );

    memset( &call, 0, sizeof(call) );
    call.provider = provider;
    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, "reentrant_submit", &call.container );
    if (SUCCEEDED( hr )) IXGameSaveImpl3_XGameSaveEnumerateBlobInfo( gs, call.container, &call, submit_from_enumeration );
    CHECK( call.called && SUCCEEDED( call.started ), "SubmitUpdateAsync starts inside a blob callback (0x%08lx)", call.started );
    CHECK( call.status == S_OK, "and completes while the callback still runs (0x%08lx)", call.status );
    if (call.status == E_PENDING) IXThreadingImpl_XAsyncGetStatus( threading, &call.async, TRUE );
    if (call.container) IXGameSaveImpl3_XGameSaveCloseContainer( gs, call.container );
    hr = read_one_blob( provider, "reentrant_submit", "inner", value, sizeof(value) );
    CHECK( SUCCEEDED( hr ) && !strcmp( value, "written" ), "the update is on disk ('%s')", value );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
}

/* ---- XGameSave: a queued save when the title quits ---------------------- */

static const char exit_save_scid[] = "00000000-0000-0000-0000-0000000000f8";

/* Child: submit a save asynchronously and quit at once, without waiting. */
static int exit_after_async_save_child( const char *container_name )
{
    XGameSaveProviderHandle provider = NULL;
    XGameSaveContainerHandle container = NULL;
    XGameSaveUpdateHandle update = NULL;
    static XAsyncBlock async;
    XUserHandle owner;

    if (!(owner = add_quiet_user())) return 100;
    if (FAILED( IXGameSaveImpl3_XGameSaveInitializeProvider( gs, owner, exit_save_scid, FALSE, &provider ) )) return 101;
    if (FAILED( IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, container_name, &container ) )) return 102;
    if (FAILED( IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, NULL, &update ) )) return 103;
    if (FAILED( IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, "last", (UINT8 *)"kept", 5 ) )) return 104;
    if (FAILED( IXGameSaveImpl3_XGameSaveSubmitUpdateAsync( gs, update, &async ) )) return 105;
    ExitProcess( 0 );
}

static void test_gamesave_queued_at_exit(void)
{
    XGameSaveProviderHandle provider = NULL;
    char args[64], name[32], value[64];
    int i, kept = 0;
    DWORD code;
    HRESULT hr;

    printf( "# XGameSave async save made just before the title quits\n" );
    for (i = 0; i < 4; i++)
    {
        snprintf( name, sizeof(name), "exit_save_%d", i );
        snprintf( args, sizeof(args), "--exit-after-async-save %s", name );
        code = run_child( args, NULL );
        CHECK( code == 0, "child %d submits and quits (%lu)", i, code );
    }
    hr = IXGameSaveImpl3_XGameSaveInitializeProvider( gs, user, exit_save_scid, FALSE, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "provider opens (0x%08lx)", hr );
    if (!provider) return;
    for (i = 0; i < 4; i++)
    {
        snprintf( name, sizeof(name), "exit_save_%d", i );
        if (SUCCEEDED( read_one_blob( provider, name, "last", value, sizeof(value) ) ) && !strcmp( value, "kept" )) kept++;
    }
    CHECK( kept == 4, "every save submitted before quitting is on disk (%d of 4)", kept );
    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
}
