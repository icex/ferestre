/*
 * Regression tests for the gameinput.dll work: the WINE_GAMEINPUT opt-in, the
 * IGameInput_v2 interface, the virtual keyboard/mouse devices and their
 * readings, and the dispatcher.
 *
 *   gameinput_tests gated     - run without WINE_GAMEINPUT: the upstream
 *                               allow-list must still refuse (E_NOTIMPL).
 *   gameinput_tests enabled   - WINE_GAMEINPUT=1: the full surface.
 */

#include <windows.h>
#include <stdio.h>
#include <string.h>

#define COBJMACROS
#include "initguid.h"
#include "gameinput.h"

static int fails;

#define OK(cond, ...) do { \
    if (cond) { printf("ok   - "); printf(__VA_ARGS__); printf("\n"); } \
    else { printf("FAIL - "); printf(__VA_ARGS__); printf("\n"); fails++; } \
} while (0)

typedef HRESULT (WINAPI *pGameInputCreate)(IGameInput_v0 **);

static int cb_count;
static IGameInputDevice_v2 *cb_device;
static GameInputDeviceStatus cb_status;

static void WINAPI device_cb( GameInputCallbackToken token, void *context, IGameInputDevice_v2 *device,
                              uint64_t timestamp, GameInputDeviceStatus current, GameInputDeviceStatus previous )
{
    cb_count++;
    cb_device = device;
    cb_status = current;
}

int main( int argc, char **argv )
{
    int gated = argc > 1 && !strcmp( argv[1], "gated" );
    const GameInputDeviceInfo_v2 *info = NULL;
    IGameInputReading_v2 *rd = NULL, *nx, *gp;
    IGameInputDevice_v2 *rdev = NULL, *found = NULL, *mouse = NULL;
    GameInputCallbackToken tok = 0, tok2 = 0, tok3 = 0, tok4 = 0;
    IGameInputDispatcher *disp = NULL;
    IUnknown *u0 = NULL, *u2 = NULL;
    IGameInput_v0 *gi0 = NULL;
    IGameInput_v2 *gi = NULL;
    GameInputMouseState_v2 ms;
    pGameInputCreate create;
    HANDLE h = NULL;
    HMODULE mod;
    HRESULT hr;

    mod = LoadLibraryA( "gameinput.dll" );
    OK( mod != NULL, "gameinput.dll loads" );
    if (!mod) goto done;
    create = (pGameInputCreate)GetProcAddress( mod, "GameInputCreate" );
    OK( create != NULL, "GameInputCreate exported" );
    if (!create) goto done;

    hr = create( &gi0 );
    if (gated)
    {
        OK( hr == E_NOTIMPL, "gated: GameInputCreate -> E_NOTIMPL, allow-list intact (got %#lx)", hr );
        goto done;
    }
    OK( hr == S_OK && gi0, "GameInputCreate -> S_OK (got %#lx)", hr );
    if (FAILED(hr)) goto done;

    hr = IGameInput_v0_QueryInterface( gi0, &IID_IGameInput_v2, (void **)&gi );
    OK( hr == S_OK && gi, "QueryInterface IGameInput_v2 -> S_OK (got %#lx)", hr );
    if (!gi) goto done;

    IGameInput_v0_QueryInterface( gi0, &IID_IUnknown, (void **)&u0 );
    IGameInput_v2_QueryInterface( gi, &IID_IUnknown, (void **)&u2 );
    OK( u0 && u0 == u2, "v0 and v2 share one IUnknown identity" );

    /* Minecraft's registration: Mouse|Gamepad, Connected, AsyncEnumeration. */
    cb_count = 0; cb_device = NULL;
    hr = IGameInput_v2_RegisterDeviceCallback( gi, NULL, GameInputKindMouse | GameInputKindGamepad, GameInputDeviceConnected,
                                               GameInputAsyncEnumeration, NULL, device_cb, &tok );
    OK( hr == S_OK && tok, "RegisterDeviceCallback(Mouse|Gamepad) -> S_OK, token set" );
    OK( cb_count == 1, "exactly one device reported (got %d)", cb_count );
    OK( cb_status == GameInputDeviceConnected, "reported as Connected (got %#x)", cb_status );
    if (cb_device) IGameInputDevice_v2_GetDeviceInfo( cb_device, &info );
    OK( info && info->supportedInput == GameInputKindMouse, "reported device is the mouse (supportedInput %#x)",
        info ? info->supportedInput : 0 );
    OK( info && info->mouseInfo && info->mouseInfo->hasWheelY, "mouse info present with a wheel" );
    OK( info && info->displayName && !strcmp( info->displayName, "Mouse" ), "displayName is \"Mouse\"" );
    OK( cb_device && (IGameInputDevice_v2_GetDeviceStatus( cb_device ) & GameInputDeviceConnected),
        "GetDeviceStatus reports Connected" );
    mouse = cb_device;

    /* Keyboard only, blocking enumeration. */
    cb_count = 0; cb_device = NULL; info = NULL;
    hr = IGameInput_v2_RegisterDeviceCallback( gi, NULL, GameInputKindKeyboard, GameInputDeviceAnyStatus_v2,
                                               GameInputBlockingEnumeration, NULL, device_cb, &tok2 );
    OK( hr == S_OK && cb_count == 1, "keyboard registration reports one device (got %d)", cb_count );
    if (cb_device) IGameInputDevice_v2_GetDeviceInfo( cb_device, &info );
    OK( info && info->keyboardInfo && info->keyboardInfo->keyCount == 104, "keyboard info present (104 keys)" );
    OK( info && info->displayName && !strcmp( info->displayName, "Keyboard" ), "displayName is \"Keyboard\"" );

    /* Gamepad only: nothing to report. */
    cb_count = 0;
    hr = IGameInput_v2_RegisterDeviceCallback( gi, NULL, GameInputKindGamepad, GameInputDeviceConnected,
                                               GameInputAsyncEnumeration, NULL, device_cb, &tok3 );
    OK( hr == S_OK && cb_count == 0, "gamepad-only registration reports nothing (got %d)", cb_count );

    /* NoEnumeration: nothing reported at registration time. */
    cb_count = 0;
    hr = IGameInput_v2_RegisterDeviceCallback( gi, NULL, GameInputKindMouse, GameInputDeviceConnected,
                                               GameInputNoEnumeration, NULL, device_cb, &tok4 );
    OK( hr == S_OK && cb_count == 0, "NoEnumeration reports nothing (got %d)", cb_count );

    /* Readings. */
    hr = IGameInput_v2_GetCurrentReading( gi, GameInputKindMouse, NULL, &rd );
    OK( hr == S_OK && rd, "GetCurrentReading(Mouse) -> S_OK (got %#lx)", hr );
    OK( rd && IGameInputReading_v2_GetInputKind( rd ) == GameInputKindMouse, "reading kind is Mouse" );
    memset( &ms, 0xff, sizeof(ms) );
    OK( rd && IGameInputReading_v2_GetMouseState( rd, &ms ) && !ms.buttons && !ms.positionX && !ms.wheelY,
        "GetMouseState -> true, nothing pressed or moved" );
    OK( rd && IGameInputReading_v2_GetTimestamp( rd ) > 0, "reading timestamp > 0" );
    if (rd) IGameInputReading_v2_GetDevice( rd, &rdev );
    OK( rdev == mouse, "reading's device is the mouse device" );

    nx = (void *)1;
    hr = IGameInput_v2_GetNextReading( gi, rd, GameInputKindMouse, NULL, &nx );
    OK( hr == GAMEINPUT_E_READING_NOT_FOUND && !nx, "GetNextReading -> READING_NOT_FOUND (got %#lx)", hr );
    gp = (void *)1;
    hr = IGameInput_v2_GetCurrentReading( gi, GameInputKindGamepad, NULL, &gp );
    OK( hr == GAMEINPUT_E_READING_NOT_FOUND && !gp, "GetCurrentReading(Gamepad) -> READING_NOT_FOUND (got %#lx)", hr );

    /* Find by id (info is the keyboard's). */
    hr = info ? IGameInput_v2_FindDeviceFromId( gi, &info->deviceId, &found ) : E_FAIL;
    OK( hr == S_OK && found == cb_device, "FindDeviceFromId resolves the keyboard" );

    /* Dispatcher. */
    hr = IGameInput_v2_CreateDispatcher( gi, &disp );
    OK( hr == S_OK && disp, "CreateDispatcher -> S_OK (got %#lx)", hr );
    OK( disp && !IGameInputDispatcher_Dispatch( disp, 0 ), "Dispatch -> false (no queued work)" );
    OK( disp && IGameInputDispatcher_OpenWaitHandle( disp, &h ) == S_OK && h, "OpenWaitHandle -> a handle" );

    OK( IGameInput_v2_UnregisterCallback( gi, tok ), "UnregisterCallback(token) -> true" );
    OK( IGameInput_v2_GetCurrentTimestamp( gi ) > 0, "GetCurrentTimestamp > 0" );

    /* Mouse motion is a running total, not a per-reading delta. The GDK
     * documents positionX/positionY as the sum of every movement delta, and a
     * caller derives motion by subtracting the previous reading's value. The
     * bug this guards against handed out the delta and reset the counter, so a
     * second read with no movement in between reported 0 instead of repeating
     * the total, and titles ended up taking a difference of differences.
     * SendInput drives the same raw-input path the runtime reads. */
    {
        IGameInputReading_v2 *mr = NULL;
        GameInputMouseState_v2 s0, s1, s2;
        INPUT in;

        memset( &s0, 0, sizeof(s0) );
        memset( &s1, 0, sizeof(s1) );
        memset( &s2, 0, sizeof(s2) );

        if (IGameInput_v2_GetCurrentReading( gi, GameInputKindMouse, NULL, &mr ) == S_OK && mr)
        {
            /* Raw input only reaches a process that owns the foreground window,
             * so without one the runtime's sink never sees the injected motion
             * and the accumulator cannot be exercised. A real title always has
             * such a window; the test has to make one. */
            HWND fg = CreateWindowExA( 0, "STATIC", "gameinput_tests", WS_OVERLAPPEDWINDOW,
                                       0, 0, 320, 240, NULL, NULL, NULL, NULL );
            MSG m;
            int p;

            ShowWindow( fg, SW_SHOW );
            SetForegroundWindow( fg );
            SetFocus( fg );
            for (p = 0; p < 60; p++)
            {
                while (PeekMessageA( &m, NULL, 0, 0, PM_REMOVE )) { TranslateMessage( &m ); DispatchMessageA( &m ); }
                Sleep( 5 );
            }
            OK( GetForegroundWindow() == fg, "test window is foreground (raw input needs one)" );

            IGameInputReading_v2_GetMouseState( mr, &s0 );

            memset( &in, 0, sizeof(in) );
            in.type = INPUT_MOUSE;
            in.mi.dx = 40;
            in.mi.dy = 25;
            in.mi.dwFlags = MOUSEEVENTF_MOVE;
            SendInput( 1, &in, sizeof(in) );
            for (p = 0; p < 100; p++)     /* let the raw-input thread drain */
            {
                while (PeekMessageA( &m, NULL, 0, 0, PM_REMOVE )) { TranslateMessage( &m ); DispatchMessageA( &m ); }
                Sleep( 5 );
            }

            IGameInputReading_v2_GetMouseState( mr, &s1 );

            if (s1.positionX == s0.positionX && s1.positionY == s0.positionY)
                printf( "ok   - # SKIP no raw mouse input reached this session; accumulator not exercised\n" );
            else
            {
                OK( s1.positionX > s0.positionX && s1.positionY > s0.positionY,
                    "injected motion shows up as a difference of two totals (%lld,%lld)",
                    (long long)(s1.positionX - s0.positionX), (long long)(s1.positionY - s0.positionY) );

                /* The heart of it: nothing moved since, so the totals repeat. */
                IGameInputReading_v2_GetMouseState( mr, &s2 );
                OK( s2.positionX == s1.positionX && s2.positionY == s1.positionY,
                    "an idle read repeats the totals instead of resetting (%lld,%lld vs %lld,%lld)",
                    (long long)s2.positionX, (long long)s2.positionY,
                    (long long)s1.positionX, (long long)s1.positionY );
            }
            IGameInputReading_v2_Release( mr );
            DestroyWindow( fg );
        }
    }

done:
    if (fails) { printf( "\nFAILED (%d)\n", fails ); return 1; }
    printf( "\nPASSED\n" );
    return 0;
}
