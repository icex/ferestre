/* Exercise the actual gameinput.dll against a deterministic XInput backend. */
#include <windows.h>
#include <xinput.h>
#include <stdio.h>
#include <string.h>
#define COBJMACROS
#include "initguid.h"
#include "gameinput.h"

static unsigned int failures, count;
static IGameInputDevice_v2 *devices[4];
#define OK(c, s) do { printf("%s - %s\n", (c) ? "ok  " : "FAIL", s); if (!(c)) failures++; } while (0)

static void WINAPI callback(GameInputCallbackToken token, void *context, IGameInputDevice_v2 *device,
                           uint64_t timestamp, GameInputDeviceStatus current, GameInputDeviceStatus previous)
{
    if (count < 4) devices[count] = device;
    count++;
}

int main(void)
{
    HRESULT (WINAPI *create)(IGameInput_v0 **);
    void (WINAPI *set_state)(DWORD, const XINPUT_STATE *, BOOL);
    void (WINAPI *get_vibration)(DWORD, XINPUT_VIBRATION *);
    HMODULE fixture = LoadLibraryA("xinput1_3.dll"), module = LoadLibraryA("gameinput.dll");
    IGameInput_v0 *v0 = NULL;
    IGameInput_v2 *input = NULL;
    IGameInputReading_v2 *reading = NULL, *next = NULL;
    IGameInputDevice_v2 *found = NULL;
    const GameInputDeviceInfo_v2 *info;
    GameInputGamepadState pad, original;
    GameInputCallbackToken token;
    GameInputRumbleParams rumble = {1.0f, 0.5f, 0, 0};
    XINPUT_VIBRATION vibration;
    XINPUT_STATE state = {7, {XINPUT_GAMEPAD_A | XINPUT_GAMEPAD_START | XINPUT_GAMEPAD_DPAD_LEFT,
                             255, 128, -32768, 32767, 0, -16384}};
    HRESULT hr;

    create = (void *)GetProcAddress(module, "GameInputCreate");
    set_state = (void *)GetProcAddress(fixture, "FixtureSetState");
    get_vibration = (void *)GetProcAddress(fixture, "FixtureGetVibration");
    OK(create && set_state && get_vibration, "real GameInput and test-only XInput backend load");
    if (!create || !set_state || !get_vibration) goto done;
    set_state(0, &state, TRUE);
    set_state(3, &state, TRUE);
    hr = create(&v0);
    OK(hr == S_OK && v0, "GameInputCreate");
    if (!v0) goto done;
    IGameInput_v0_QueryInterface(v0, &IID_IGameInput_v2, (void **)&input);
    OK(input != NULL, "IGameInput_v2 available");
    if (!input) goto done;
    hr = IGameInput_v2_RegisterDeviceCallback(input, NULL, GameInputKindGamepad, 0x200001,
                                             GameInputBlockingEnumeration, NULL, callback, &token);
    OK(hr == S_OK && count == 2, "enumerates exactly the two connected pads, including slot 3");
    if (count != 2) goto done;
    IGameInputDevice_v2_GetDeviceInfo(devices[0], &info);
    OK(info->supportedInput == GameInputKindGamepad && info->gamepadInfo, "gamepad metadata present");
    IGameInput_v2_FindDeviceFromId(input, &info->deviceId, &found);
    OK(found == devices[0], "FindDeviceFromId preserves gamepad identity");
    if (found) IGameInputDevice_v2_Release(found);
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &reading);
    OK(hr == S_OK && reading, "connected pad supplies a reading");
    if (!reading) goto done;
    OK(IGameInputReading_v2_GetGamepadState(reading, &pad), "reading supplies gamepad state");
    OK(pad.buttons == (GameInputGamepadA | GameInputGamepadMenu | GameInputGamepadDPadLeft), "button mapping");
    OK(pad.leftTrigger == 1.0f && pad.rightTrigger > 0.501f && pad.rightTrigger < 0.503f, "trigger normalization");
    OK(pad.leftThumbstickX == -1.0f && pad.leftThumbstickY == 1.0f && pad.rightThumbstickY < -0.49f,
       "signed stick endpoints and direction");
    original = pad;
    hr = IGameInput_v2_GetNextReading(input, reading, GameInputKindGamepad, devices[0], &next);
    OK(hr == GAMEINPUT_E_READING_NOT_FOUND && !next, "unchanged packet terminates a drain loop");
    state.dwPacketNumber++;
    state.Gamepad.wButtons = XINPUT_GAMEPAD_B;
    set_state(0, &state, TRUE);
    hr = IGameInput_v2_GetNextReading(input, reading, GameInputKindGamepad, devices[0], &next);
    OK(hr == S_OK && next, "changed packet supplies next reading");
    if (next)
    {
        IGameInputReading_v2_GetGamepadState(next, &pad);
        OK(pad.buttons == GameInputGamepadB, "next reading contains the new input");
        IGameInputReading_v2_Release(next);
    }
    IGameInputReading_v2_GetGamepadState(reading, &pad);
    OK(!memcmp(&pad, &original, sizeof(pad)), "retained reading stays immutable");
    IGameInputDevice_v2_SetRumbleState(devices[0], &rumble);
    get_vibration(0, &vibration);
    OK(vibration.wLeftMotorSpeed == 65535 && vibration.wRightMotorSpeed == 32767, "rumble reaches XInput");
    set_state(0, &state, FALSE);
    OK(IGameInputDevice_v2_GetDeviceStatus(devices[0]) == GameInputDeviceNoStatus, "disconnected pad reports disconnected");
    next = NULL;
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &next);
    OK(hr == GAMEINPUT_E_READING_NOT_FOUND && !next, "disconnected pad supplies no reading");
    IGameInput_v2_UnregisterCallback(input, token);

done:
    if (reading) IGameInputReading_v2_Release(reading);
    if (input) IGameInput_v2_Release(input);
    if (v0) IGameInput_v0_Release(v0);
    printf("%s (%u failures)\n", failures ? "FAILED" : "PASSED", failures);
    return failures != 0;
}
