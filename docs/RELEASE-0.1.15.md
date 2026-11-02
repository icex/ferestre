# Ferestre 0.1.15

Fixes game installs and updates failing because Xbox rejects the sign-in ticket format. The bundled Xbox client and service now retry a rejected delegation ticket with the format Xbox accepts. Existing sign-ins can be reused.

The retry happens once, only after HTTP 400 for a delegation ticket. Successful requests and other errors keep their existing behavior. Proof-bound requests are signed again for the corrected body; stored credentials keep their original format.

The AUR client package includes the same fix. This release uses the existing [runtime 11.0.20261023.1](https://github.com/icex/ferestre/releases/tag/runtime-11.0.20261023.1); no runtime update is needed for this fix.

## Verification

- All eight authentication tests passed, including signed retries, retry limits, successful original requests, and other HTTP failures. These tests now run in hosted CI.
- Live sign-in and update checks succeeded for all seven installed games. An isolated retail update downloaded and decrypted over 2 GB before the temporary test was stopped and removed. Full update completion was not verified in that test; the local user subsequently reported that updating works.
