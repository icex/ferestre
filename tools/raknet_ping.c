/*
 * Does Bedrock's multiplayer transport work under Wine?
 *
 * Minecraft Bedrock talks RakNet over UDP, which never touches the HTTP path
 * the XCurl shim traces, so a join that hangs at "Locating server" cannot be
 * explained -- or ruled out -- from HTTP logs alone. This sends RakNet's
 * unconnected ping to a server and waits for the pong, which is exactly the
 * exchange the client makes before it connects.
 *
 *   raknet_ping.exe <host> <port>
 *
 * Build (Proton SDK container):
 *   x86_64-w64-mingw32-gcc -O2 -o raknet_ping.exe raknet_ping.c -lws2_32
 */
#include <winsock2.h>
#include <ws2tcpip.h>
#include <stdio.h>
#include <string.h>

static const unsigned char MAGIC[16] = {
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe,
    0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78
};

int main( int argc, char **argv )
{
    const char *host = argc > 1 ? argv[1] : "geo.hivebedrock.cloud";
    const char *port = argc > 2 ? argv[2] : "19132";
    unsigned char ping[33], buf[2048];
    struct addrinfo hints = { 0 }, *res = NULL;
    WSADATA wsa;
    SOCKET s;
    int n, tries;
    DWORD timeout = 3000;

    if (WSAStartup( MAKEWORD(2,2), &wsa )) { printf( "WSAStartup failed\n" ); return 2; }

    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_DGRAM;
    hints.ai_protocol = IPPROTO_UDP;
    /* The port is a number, not a service name: without this Wine's resolver
     * looks "19132" up in the services list and reports host-not-found, which
     * looks exactly like a DNS failure and is not one. */
    hints.ai_flags = AI_NUMERICSERV;
    if (getaddrinfo( host, port, &hints, &res ) || !res)
    { printf( "%s:%s -> DNS FAILED (%d)\n", host, port, WSAGetLastError() ); return 2; }

    if ((s = socket( AF_INET, SOCK_DGRAM, IPPROTO_UDP )) == INVALID_SOCKET)
    { printf( "socket failed %d\n", WSAGetLastError() ); return 2; }
    setsockopt( s, SOL_SOCKET, SO_RCVTIMEO, (const char *)&timeout, sizeof(timeout) );

    /* ID_UNCONNECTED_PING, 8-byte time, magic, 8-byte client guid */
    ping[0] = 0x01;
    memset( ping + 1, 0, 8 );
    ping[8] = 0x39;                       /* any non-zero time */
    memcpy( ping + 9, MAGIC, 16 );
    memset( ping + 25, 0x11, 8 );

    /* Optional third argument: probe RakNet's MTU discovery instead of pinging.
     *
     * A connect begins with OpenConnectionRequest1, padded to the MTU the
     * client wants to use, and the server replies only if the whole datagram
     * arrives. A join that completes its handshake and then stalls while chunks
     * stream is exactly what a silently-dropped large datagram looks like, so
     * this reports which sizes survive the trip. */
    if (argc > 3 && !strcmp( argv[3], "mtu" ))
    {
        static const int sizes[] = { 1492, 1400, 1200, 1024, 576, 400 };
        unsigned int i;

        for (i = 0; i < sizeof(sizes)/sizeof(sizes[0]); i++)
        {
            unsigned char req[1600];
            int size = sizes[i];

            memset( req, 0, sizeof(req) );
            req[0] = 0x05;                    /* OpenConnectionRequest1 */
            memcpy( req + 1, MAGIC, 16 );
            req[17] = 11;                     /* RakNet protocol version */
            /* the rest is zero padding, and its length is the MTU probe */

            if (sendto( s, (const char *)req, size, 0, res->ai_addr, (int)res->ai_addrlen ) == SOCKET_ERROR)
            {
                printf( "  MTU %4d -> SEND FAILED %d\n", size, WSAGetLastError() );
                continue;
            }
            n = recvfrom( s, (char *)buf, sizeof(buf), 0, NULL, NULL );
            if (n > 0)
                printf( "  MTU %4d -> reply id 0x%02x, %d bytes\n", size, buf[0], n );
            else
                printf( "  MTU %4d -> no reply (error %d)\n", size, WSAGetLastError() );
        }
        return 0;
    }

    /* "connect": walk RakNet's whole offline handshake, not just its first
     * packet. Request1 discovers the MTU, Request2 agrees the session. If
     * Reply2 comes back the connection is established as far as RakNet is
     * concerned, and anything that hangs after that is above this layer. */
    if (argc > 3 && !strcmp( argv[3], "connect" ))
    {
        unsigned char req[1600];
        int mtu = 1400;

        memset( req, 0, sizeof(req) );
        req[0] = 0x05;
        memcpy( req + 1, MAGIC, 16 );
        req[17] = 11;
        if (sendto( s, (const char *)req, mtu, 0, res->ai_addr, (int)res->ai_addrlen ) == SOCKET_ERROR)
        { printf( "Request1 send failed %d\n", WSAGetLastError() ); return 2; }
        n = recvfrom( s, (char *)buf, sizeof(buf), 0, NULL, NULL );
        if (n <= 0 || buf[0] != 0x06)
        { printf( "Request1 -> no OpenConnectionReply1 (got %d bytes, error %d)\n", n, WSAGetLastError() ); return 1; }
        {
            int server_mtu = (buf[n-2] << 8) | buf[n-1];
            printf( "Reply1  ok: server offers MTU %d, security byte %d\n", server_mtu, buf[25] );
            if (server_mtu > 0 && server_mtu < 1500) mtu = server_mtu;
        }

        /* Request2: magic, server address, MTU, client GUID */
        memset( req, 0, sizeof(req) );
        req[0] = 0x07;
        memcpy( req + 1, MAGIC, 16 );
        {
            struct sockaddr_in *sin = (struct sockaddr_in *)res->ai_addr;
            unsigned char *p = req + 17;
            *p++ = 4;                                   /* IPv4 */
            memcpy( p, &sin->sin_addr, 4 ); p += 4;     /* address, RakNet stores it complemented */
            p[-4] = ~p[-4]; p[-3] = ~p[-3]; p[-2] = ~p[-2]; p[-1] = ~p[-1];
            *p++ = (unsigned char)(ntohs( sin->sin_port ) >> 8);
            *p++ = (unsigned char)(ntohs( sin->sin_port ) & 0xff);
            *p++ = (unsigned char)(mtu >> 8);
            *p++ = (unsigned char)(mtu & 0xff);
            memset( p, 0x22, 8 );                       /* client guid */
            p += 8;
            n = (int)(p - req);
        }
        if (sendto( s, (const char *)req, n, 0, res->ai_addr, (int)res->ai_addrlen ) == SOCKET_ERROR)
        { printf( "Request2 send failed %d\n", WSAGetLastError() ); return 2; }
        n = recvfrom( s, (char *)buf, sizeof(buf), 0, NULL, NULL );
        if (n > 0 && buf[0] == 0x08)
        { printf( "Reply2  ok: %d bytes -- RakNet session established\n", n ); return 0; }
        printf( "Request2 -> no OpenConnectionReply2 (got %d bytes, id 0x%02x, error %d)\n",
                n, n > 0 ? buf[0] : 0, WSAGetLastError() );
        return 1;
    }

    for (tries = 0; tries < 3; tries++)
    {
        if (sendto( s, (const char *)ping, sizeof(ping), 0, res->ai_addr, (int)res->ai_addrlen ) == SOCKET_ERROR)
        { printf( "%s:%s -> SEND FAILED %d\n", host, port, WSAGetLastError() ); return 2; }

        n = recvfrom( s, (char *)buf, sizeof(buf), 0, NULL, NULL );
        if (n > 0)
        {
            if (buf[0] == 0x1c && n > 35)
            {
                /* the pong carries a semicolon-separated MOTD after a length */
                int off = 1 + 8 + 8 + 16 + 2;
                int len = n - off;
                if (len > 120) len = 120;
                printf( "%s:%s -> PONG %d bytes: %.*s\n", host, port, n, len, (char *)buf + off );
            }
            else printf( "%s:%s -> reply %d bytes, id 0x%02x\n", host, port, n, buf[0] );
            return 0;
        }
    }
    printf( "%s:%s -> NO REPLY (last error %d)\n", host, port, WSAGetLastError() );
    return 1;
}
