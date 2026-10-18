/* XNetworkingVerifyServerCertificate against WinHTTP requests to a local
 * fixture (tests/https_fixture.py). Synthetic certificates only; nothing here
 * leaves the machine.
 *
 *   xnetworking_cert.exe <https port> <http port> <test CA, DER, Windows path>
 *
 * The call is made where libHttpClient makes it, from the request's
 * WINHTTP_CALLBACK_STATUS_SENDING_REQUEST callback. It used to accept every
 * certificate; now it must verify the chain against the request's host, and
 * honour the exceptions the request itself carries. */
#define COBJMACROS
#include <windows.h>
#include <winhttp.h>
#include <wincrypt.h>
#include <initguid.h>
#include <stdio.h>
#include <stdlib.h>
#include "xnetworking.h"

static IXNetworkingImpl2 *net;
static unsigned int failures, checks;
#define CHECK(c, ...) do { checks++; if (c) printf("ok: "); else { printf("FAIL: "); failures++; } \
                           printf(__VA_ARGS__); printf("\n"); } while (0)

struct probe
{
    DWORD clear_flags;      /* SECURITY_FLAGS to set just before verifying, ~0u to leave them */
    HRESULT hr;
    BOOL called;
};

static void CALLBACK status_callback(HINTERNET request, DWORD_PTR context, DWORD status, void *info, DWORD len)
{
    struct probe *probe = (struct probe *)context;
    DWORD flags;

    if (status != WINHTTP_CALLBACK_STATUS_SENDING_REQUEST || !probe) return;
    if (probe->clear_flags != ~0u)
    {
        flags = probe->clear_flags;
        WinHttpSetOption(request, WINHTTP_OPTION_SECURITY_FLAGS, &flags, sizeof(flags));
    }
    probe->hr = IXNetworkingImpl2_XNetworkingVerifyServerCertificate(net, request, NULL);
    probe->called = TRUE;
}

/* One GET; returns the verdict given while it was being sent. */
static HRESULT request(const WCHAR *host, unsigned int port, BOOL secure, DWORD flags, DWORD clear_flags, BOOL *received)
{
    HINTERNET session, connection, req;
    struct probe probe = { clear_flags, E_UNEXPECTED, FALSE };

    *received = FALSE;
    session = WinHttpOpen(L"ferestre-test", WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    connection = session ? WinHttpConnect(session, host, port, 0) : NULL;
    req = connection ? WinHttpOpenRequest(connection, L"GET", L"/", NULL, WINHTTP_NO_REFERER,
                                          WINHTTP_DEFAULT_ACCEPT_TYPES, secure ? WINHTTP_FLAG_SECURE : 0) : NULL;
    if (req)
    {
        if (flags) WinHttpSetOption(req, WINHTTP_OPTION_SECURITY_FLAGS, &flags, sizeof(flags));
        WinHttpSetStatusCallback(req, status_callback, WINHTTP_CALLBACK_FLAG_SEND_REQUEST, 0);
        if (WinHttpSendRequest(req, WINHTTP_NO_ADDITIONAL_HEADERS, 0, NULL, 0, 0, (DWORD_PTR)&probe))
            *received = WinHttpReceiveResponse(req, NULL);
        else printf("     send to %ls:%u failed: %lu\n", host, port, GetLastError());
        WinHttpCloseHandle(req);
    }
    if (connection) WinHttpCloseHandle(connection);
    if (session) WinHttpCloseHandle(session);
    if (!probe.called) printf("     the sending-request callback did not run\n");
    return probe.called ? probe.hr : E_UNEXPECTED;
}

/* Trust the fixture's CA in this throwaway prefix. */
static BOOL trust_test_ca(const char *path)
{
    BYTE der[8192];
    HCERTSTORE root;
    DWORD size;
    FILE *file = fopen(path, "rb");
    BOOL ok;

    if (!file) return FALSE;
    size = fread(der, 1, sizeof(der), file);
    fclose(file);
    if (!(root = CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER, L"Root")))
        return FALSE;
    ok = CertAddEncodedCertificateToStore(root, X509_ASN_ENCODING, der, size, CERT_STORE_ADD_REPLACE_EXISTING, NULL);
    CertCloseStore(root, 0);
    return ok;
}

int main(int argc, char **argv)
{
    static const DWORD ignore_ca = SECURITY_FLAG_IGNORE_UNKNOWN_CA;
    HRESULT (WINAPI *query)(REFCLSID, REFIID, void **);
    unsigned int https_port, http_port;
    HMODULE runtime;
    BOOL received;
    HRESULT hr;

    if (argc < 4) { printf("usage: xnetworking_cert <https port> <http port> <ca.der>\n"); return 2; }
    https_port = atoi(argv[1]);
    http_port = atoi(argv[2]);
    if (!(runtime = LoadLibraryA("xgameruntime.dll"))) return 11;
    query = (void *)GetProcAddress(runtime, "QueryApiImpl");
    if (!query || FAILED(query(&CLSID_XNetworkingImpl, &IID_IXNetworkingImpl2, (void **)&net))) return 12;

    /* Nothing that is not a WinHTTP request can be checked: the old answer. */
    hr = IXNetworkingImpl2_XNetworkingVerifyServerCertificate(net, NULL, NULL);
    CHECK(hr == S_OK, "no request handle keeps the old answer (%#lx)", hr);
    LoadLibraryA("winhttp.dll");
    hr = IXNetworkingImpl2_XNetworkingVerifyServerCertificate(net, (void *)(ULONG_PTR)0x1234, NULL);
    CHECK(hr == S_OK, "a handle WinHTTP does not know keeps the old answer (%#lx)", hr);

    hr = request(L"localhost", http_port, FALSE, 0, ~0u, &received);
    CHECK(hr == S_OK && received, "a plain HTTP request has no certificate to check (%#lx)", hr);

    /* The CA is not trusted yet. WinHTTP was told to accept it anyway, and so
     * must this be; told nothing, both refuse it. */
    hr = request(L"localhost", https_port, TRUE, ignore_ca, ~0u, &received);
    CHECK(hr == S_OK && received, "a certificate the request was told to accept is accepted (%#lx)", hr);
    hr = request(L"localhost", https_port, TRUE, ignore_ca, 0, &received);
    CHECK(hr == HRESULT_FROM_WIN32(ERROR_WINHTTP_SECURE_INVALID_CA),
          "an untrusted chain is refused (%#lx)", hr);

    if (!trust_test_ca(argv[3]))
    {
        CHECK(0, "the test CA could be trusted in the scratch prefix (%lu)", GetLastError());
        printf("%s (%u checks, %u failures)\n", failures ? "FAILED" : "PASSED", checks, failures);
        return 1;
    }
    hr = request(L"localhost", https_port, TRUE, 0, ~0u, &received);
    CHECK(hr == S_OK && received, "a trusted chain for the request's host is accepted (%#lx)", hr);
    /* The certificate names localhost, not 127.0.0.1. */
    hr = request(L"127.0.0.1", https_port, TRUE, SECURITY_FLAG_IGNORE_CERT_CN_INVALID, ~0u, &received);
    CHECK(hr == S_OK && received, "a name mismatch the request was told to accept is accepted (%#lx)", hr);
    hr = request(L"127.0.0.1", https_port, TRUE, SECURITY_FLAG_IGNORE_CERT_CN_INVALID, 0, &received);
    CHECK(hr == HRESULT_FROM_WIN32(ERROR_WINHTTP_SECURE_CERT_CN_INVALID),
          "a certificate for another host is refused (%#lx)", hr);

    printf("%s (%u checks, %u failures)\n", failures ? "FAILED" : "PASSED", checks, failures);
    return failures ? 1 : 0;
}
