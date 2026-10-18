#!/bin/bash
# XNetworkingVerifyServerCertificate against real WinHTTP requests to a
# loopback fixture (tests/https_fixture.py) with a synthetic CA made here.
#
#   tests/run-xnetworking-tests.sh
#
# Env overrides as the other suites: BUILD_DIR, TOOL_DIR, and XNET_TEST_DLL /
# XNET_TEST_UNIXLIB for an xgameruntime built elsewhere. Needs openssl.
set -euo pipefail
REPO_DIR=${REPO_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
. "$REPO_DIR/scripts/xodus-env.sh"
BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
OBJ=$BUILD_DIR/obj-wine-x86_64
TOOL_DIR=${TOOL_DIR:?Set TOOL_DIR to the runtime base to test}
command -v openssl >/dev/null || { echo "openssl is needed to make the test certificates" >&2; exit 2; }
WORK=$(mktemp -d "$BUILD_DIR/.xnetworking-tests.XXXXXX")
cleanup() {
    if [ -n "${WINE:-}" ]; then WINEPREFIX="$WORK/prefix" "$WINE"server -k 2>/dev/null || true; fi
    if [ -n "${FIXTURE:-}" ]; then kill -TERM "$FIXTURE" 2>/dev/null || true; wait "$FIXTURE" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

"$REPO_DIR/tools/in-container.sh" x86_64-w64-mingw32-gcc -O1 -D__WINESRC__ \
    -I"$OBJ/dlls/xgameruntime" -I"$BUILD_DIR/src-wine/dlls/xgameruntime" \
    -o "$WORK/xnetworking_cert.exe" "$REPO_DIR/tests/xnetworking_cert.c" -lwinhttp -lcrypt32 -luuid

# A CA, and a server certificate from it that names localhost only.
C=$WORK/certs
mkdir -p "$C"
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$C/ca.key" -out "$C/ca.pem" -days 2 \
    -subj "/CN=Ferestre test CA" -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout "$C/server.key" -out "$C/server.csr" -subj "/CN=localhost" 2>/dev/null
printf '%s\n' "subjectAltName=DNS:localhost" "extendedKeyUsage=serverAuth" "basicConstraints=CA:FALSE" \
    "keyUsage=digitalSignature,keyEncipherment" > "$C/server.ext"
openssl x509 -req -in "$C/server.csr" -CA "$C/ca.pem" -CAkey "$C/ca.key" -CAcreateserial \
    -out "$C/server.pem" -days 2 -extfile "$C/server.ext" 2>/dev/null
openssl x509 -in "$C/ca.pem" -outform DER -out "$WORK/ca.der"

# A second CA that is never trusted, sent by the server after the certificate
# it issued, so the chain reaches it (an untrusted root, not a partial chain)
# and its revocation list can be looked up. The list is a local distribution
# point: the runtime only consults what is already on the machine.
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$C/ca2.key" -out "$C/ca2.pem" -days 2 \
    -subj "/CN=Ferestre untrusted test CA" -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
CRL_URL="file:///Z:$C/ca2.crl"  # Wine's cryptnet opens file URLs only with a drive
issue2() {  # name, extended key usage, x509 dates...
    local name=$1 eku=$2; shift 2
    openssl req -newkey rsa:2048 -nodes -keyout "$C/$name.key" -out "$C/$name.csr" -subj "/CN=localhost" 2>/dev/null
    printf '%s\n' "subjectAltName=DNS:localhost" "extendedKeyUsage=$eku" "basicConstraints=CA:FALSE" \
        "keyUsage=digitalSignature,keyEncipherment" "crlDistributionPoints=URI:$CRL_URL" > "$C/$name.ext"
    openssl x509 -req -in "$C/$name.csr" -CA "$C/ca2.pem" -CAkey "$C/ca2.key" -CAcreateserial \
        -out "$C/$name.leaf.pem" -extfile "$C/$name.ext" "$@" 2>/dev/null
    cat "$C/$name.leaf.pem" "$C/ca2.pem" > "$C/$name.pem"
}
# Wrong on every count it can be told to ignore: expired, for client
# authentication only, and (asked for as 127.0.0.1) for another host.
issue2 expired clientAuth -not_before 20200101000000Z -not_after 20200102000000Z
# Revoked, and otherwise only from the untrusted CA.
issue2 revoked serverAuth -days 2
printf '%s\n' "[ca]" "default_ca=test" "[test]" "database=$C/index.txt" "crlnumber=$C/crlnumber" \
    "default_md=sha256" "default_crl_days=2" > "$C/ca2.cnf"
: > "$C/index.txt"; echo 01 > "$C/crlnumber"
openssl ca -config "$C/ca2.cnf" -cert "$C/ca2.pem" -keyfile "$C/ca2.key" -revoke "$C/revoked.leaf.pem" 2>/dev/null
openssl ca -config "$C/ca2.cnf" -cert "$C/ca2.pem" -keyfile "$C/ca2.key" -gencrl -out "$C/ca2.crl.pem" 2>/dev/null
openssl crl -in "$C/ca2.crl.pem" -outform DER -out "$C/ca2.crl"

cp -a --reflink=auto "$TOOL_DIR/files" "$WORK/wine"
rm -f "$WORK/wine/lib/wine/x86_64-windows/xgameruntime.dll" "$WORK/wine/lib/wine/x86_64-unix/xgameruntime.so"
cp "${XNET_TEST_DLL:-$OBJ/dlls/xgameruntime/x86_64-windows/xgameruntime.dll}" "$WORK/wine/lib/wine/x86_64-windows/xgameruntime.dll"
cp "${XNET_TEST_UNIXLIB:-$OBJ/dlls/xgameruntime/xgameruntime.so}" "$WORK/wine/lib/wine/x86_64-unix/xgameruntime.so"
WINE=$WORK/wine/bin/wine

# Nothing here may reach the real service, an account, or a proxy.
unset XODUS_STORE_PROFILE XODUS_PLAYER_PROFILE XODUS_ACCOUNT_PROFILE FERESTRE_ACCOUNT_ROLES \
    WINE_PACKAGE_MANIFEST WINEDLLOVERRIDES http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY
mkdir -p "$WORK/rt"
export XODUS_RUNTIME_DIR=$WORK/rt
export WINEPREFIX=$WORK/prefix WINEDEBUG=-all

python3 "$REPO_DIR/tests/https_fixture.py" "$WORK/ports" "$C/server.pem" "$C/server.key" \
    "$C/expired.pem" "$C/expired.key" "$C/revoked.pem" "$C/revoked.key" &
FIXTURE=$!
for ((i=0;i<100;i++)); do [ -s "$WORK/ports" ] && break; sleep 0.05; done
[ -s "$WORK/ports" ] || { echo "the HTTPS fixture did not start" >&2; exit 1; }
read -r HTTP_PORT HTTPS_PORT EXPIRED_PORT REVOKED_PORT < "$WORK/ports"

CA_WIN="Z:$(echo "$WORK/ca.der" | sed 's#/#\\#g')"
(cd "$WORK" && timeout 120 "$WINE" "$WORK/xnetworking_cert.exe" "$HTTPS_PORT" "$HTTP_PORT" "$CA_WIN" \
    "$EXPIRED_PORT" "$REVOKED_PORT") 2>/dev/null \
    | grep -avE '^ntsync|radv is not|^wine:|wineserver' | tee "$WORK/result.txt"
grep -q '^PASSED' "$WORK/result.txt"
