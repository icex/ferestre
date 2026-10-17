/* Test-only XInput backend. Never installed into a runtime. */
#include <windows.h>
#include <xinput.h>

static XINPUT_STATE states[XUSER_MAX_COUNT];
static BOOL connected[XUSER_MAX_COUNT];
static XINPUT_VIBRATION vibration[XUSER_MAX_COUNT];

__declspec(dllexport) void WINAPI FixtureSetState(DWORD index, const XINPUT_STATE *state, BOOL present)
{
    if (index >= XUSER_MAX_COUNT) return;
    states[index] = *state;
    connected[index] = present;
}

__declspec(dllexport) void WINAPI FixtureGetVibration(DWORD index, XINPUT_VIBRATION *out)
{
    if (index < XUSER_MAX_COUNT) *out = vibration[index];
}

__declspec(dllexport) DWORD WINAPI XInputGetState(DWORD index, XINPUT_STATE *state)
{
    if (index >= XUSER_MAX_COUNT || !connected[index]) return ERROR_DEVICE_NOT_CONNECTED;
    *state = states[index];
    return ERROR_SUCCESS;
}

__declspec(dllexport) DWORD WINAPI XInputSetState(DWORD index, XINPUT_VIBRATION *state)
{
    if (index >= XUSER_MAX_COUNT || !connected[index]) return ERROR_DEVICE_NOT_CONNECTED;
    vibration[index] = *state;
    return ERROR_SUCCESS;
}
