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
    const char *expected = getenv( "XGR_EXPECTED_FAMILY" );
    struct enum_ctx ctx = { 0 };
    XAsyncBlock async = { 0 };
    XPackageMountHandle mount = NULL;
    XPackageChunkAvailability avail = 99;
    XPackageInstallationMonitorHandle mon = NULL;
    XPackageInstallationProgress prog = { 0 };
    char buf[512];
    SIZE_T size = 0;
    HRESULT hr;

    printf( "# XPackage\n" );
    CHECK( IXPackageImpl3_XPackageIsPackagedProcess( pkg ), "IsPackagedProcess is TRUE" );

    hr = IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier( pkg, sizeof(buf), buf );
    CHECK( SUCCEEDED( hr ), "GetCurrentProcessPackageIdentifier succeeds (0x%08lx)", hr );
    if (expected)
        CHECK( !strcmp( buf, expected ), "identifier '%s' == expected '%s'", buf, expected );

    hr = IXPackageImpl3_XPackageGetUserLocale( pkg, sizeof(buf), buf );
    CHECK( SUCCEEDED( hr ) && buf[0], "GetUserLocale returns a locale ('%s')", buf );

    hr = IXPackageImpl3_XPackageEnumeratePackages( pkg, XPackageKind_Game,
            XPackageEnumerationScope_ThisOnly, &ctx, count_package );
    CHECK( SUCCEEDED( hr ) && ctx.count == 1, "EnumeratePackages(Game) reports exactly 1 (%d)", ctx.count );
    if (expected)
        CHECK( !strcmp( ctx.identifier, expected ), "enumerated identifier matches" );
    CHECK( !strcmp( ctx.title, "4D5E6F70" ), "enumerated titleID == fixture ('%s')", ctx.title );
    CHECK( ctx.version.major == 2 && ctx.version.minor == 7 && ctx.version.build == 13 &&
           ctx.version.revision == 42, "enumerated version == 2.7.13.42 (%u.%u.%u.%u)",
           ctx.version.major, ctx.version.minor, ctx.version.build, ctx.version.revision );

    ctx.count = 0;
    hr = IXPackageImpl3_XPackageEnumeratePackages( pkg, XPackageKind_Content,
            XPackageEnumerationScope_ThisOnly, &ctx, count_package );
    CHECK( SUCCEEDED( hr ) && ctx.count == 0, "EnumeratePackages(Content) reports 0 (%d)", ctx.count );

    hr = IXPackageImpl3_XPackageMountWithUiAsync( pkg, buf, &async );
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

    hr = IXPackageImpl3_XPackageFindChunkAvailability( pkg, buf, 0, NULL, &avail );
    CHECK( SUCCEEDED( hr ) && avail == XPackageChunkAvailability_Ready,
           "FindChunkAvailability is Ready (%d)", avail );

    hr = IXPackageImpl3_XPackageCreateInstallationMonitor( pkg, buf, 0, NULL, 0, NULL, &mon );
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
    XUserState state = 99;
    XUserLocalId local = { 0 };
    BOOLEAN guest = TRUE;
    UINT64 id = 1;
    HRESULT hr;

    printf( "# XUser\n" );
    hr = IXUserImpl_XUserAddAsync( users, XUserAddOptions_AddDefaultUserSilently, &async );
    CHECK( SUCCEEDED( hr ), "XUserAddAsync succeeds (0x%08lx)", hr );
    hr = IXUserImpl_XUserAddResult( users, &async, &user );
    CHECK( SUCCEEDED( hr ) && user, "XUserAddResult returns a user (0x%08lx)", hr );
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
    XGameSaveProviderHandle provider = NULL;
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
    hr = IXGameSaveImpl3_XGameSaveInitializeProviderResult( gs, &async, &provider );
    CHECK( SUCCEEDED( hr ) && provider, "InitializeProvider succeeds (0x%08lx)", hr );
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

    /* licence token: empty but valid-sized */
    memset( &async, 0, sizeof(async) );
    IXStoreImpl6_XStoreQueryLicenseTokenAsync( store, ctx, NULL, 0, "test", &async );
    hr = IXStoreImpl6_XStoreQueryLicenseTokenResultSize( store, &async, &size );
    CHECK( SUCCEEDED( hr ) && size >= 1, "licence token size >= 1 (%zu)", (size_t)size );
    hr = IXStoreImpl6_XStoreQueryLicenseTokenResult( store, &async, sizeof(buf), buf );
    CHECK( SUCCEEDED( hr ), "licence token result succeeds (0x%08lx)", hr );

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
            IXThreadingImpl_XAsyncComplete( threading, &block, S_OK, 0 );
            for (waited = 0; waited < 200 && !completion_ran; waited++) Sleep( 10 );
            CHECK( completion_ran,
                   "a Manual-queue completion is delivered while nothing pumps the queue" );

            IXThreadingImpl_XTaskQueueSetCurrentProcessTaskQueue( threading, saved );
            IXThreadingImpl_XTaskQueueCloseHandle( threading, manual );
        }
        if (saved) IXThreadingImpl_XTaskQueueCloseHandle( threading, saved );
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

int main(void)
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

    test_xpackage();
    test_xuser();
    test_xstore();
    test_xgameui();
    test_storage_and_system();
    test_xthreading();
    test_xgamesave( scid );

    printf( "\n1..%d\n", g_run );
    printf( "%s: %d checks, %d failed\n", g_failed ? "FAILED" : "PASSED", g_run, g_failed );
    return g_failed;
}
