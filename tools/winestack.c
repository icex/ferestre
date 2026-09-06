#define _GNU_SOURCE
/*
 * winestack - conservative stack scanner for Wine processes.
 *
 * gdb hangs enumerating threads in a wedged Wine process and winedbg faults
 * internally, so neither can tell us where a frozen game thread is parked.
 * eu-stack unwinds only the unix side: Wine's syscall dispatcher restores the
 * unix frame, so a PE thread's real call chain is invisible to a CFI unwinder.
 *
 * This walks the raw stack instead and reports every value that lands in an
 * executable mapping.  Stale slots are reported too - it is a conservative
 * scan, not a true unwind - but that is enough to answer "which module is this
 * thread waiting inside?", which is the question that matters.
 *
 * Usage: winestack <pid> [tid ...]      (no tid = every thread)
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dirent.h>
#include <errno.h>
#include <sys/ptrace.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <sys/user.h>
#include <elf.h>

struct region {
    unsigned long start, end, offset;
    int exec;
    char path[512];
};

static struct region regions[8192];
static int region_count;

static void load_regions( int pid )
{
    char name[64];
    char line[1024];
    FILE *f;

    snprintf( name, sizeof(name), "/proc/%d/maps", pid );
    if (!(f = fopen( name, "r" ))) { perror( "maps" ); exit( 1 ); }

    while (fgets( line, sizeof(line), f ) && region_count < 8192)
    {
        struct region *r = &regions[region_count];
        char perms[8];
        int n = sscanf( line, "%lx-%lx %7s %lx %*s %*s %511[^\n]",
                        &r->start, &r->end, perms, &r->offset, r->path );
        if (n < 4) continue;
        if (n < 5) r->path[0] = 0;
        /* leading spaces survive the %[^\n] conversion */
        if (r->path[0])
        {
            char *p = r->path;
            while (*p == ' ') p++;
            memmove( r->path, p, strlen( p ) + 1 );
        }
        r->exec = (perms[2] == 'x');
        region_count++;
    }
    fclose( f );
}

/* Module base is the lowest mapping of the same file, so addr - base is the
 * RVA - directly comparable with objdump output for our own DLLs. */
static unsigned long module_base( const char *path )
{
    unsigned long base = ~0UL;
    int i;

    for (i = 0; i < region_count; i++)
        if (regions[i].path[0] && !strcmp( regions[i].path, path ) && regions[i].start < base)
            base = regions[i].start;
    return base;
}

static const struct region *find_exec( unsigned long addr )
{
    int i;

    for (i = 0; i < region_count; i++)
        if (regions[i].exec && addr >= regions[i].start && addr < regions[i].end)
            return &regions[i];
    return NULL;
}

/* Wine maps a PE's header page from the file and its .text anonymously one page
 * higher, so a code address usually lands in a region with no path at all.
 * Naming it after the region start then reports an RVA one section-alignment
 * too low -- which silently pointed several frames at the wrong functions, and
 * mid-instruction at that. The module is the mapping that ends where the
 * anonymous one begins. */
static const struct region *module_of( const struct region *exec )
{
    int i;

    if (exec->path[0]) return exec;
    for (i = 0; i < region_count; i++)
        if (regions[i].path[0] && regions[i].end == exec->start) return &regions[i];
    return NULL;
}

static const struct region *find_any( unsigned long addr )
{
    int i;

    for (i = 0; i < region_count; i++)
        if (addr >= regions[i].start && addr < regions[i].end)
            return &regions[i];
    return NULL;
}

static void describe_inline( unsigned long addr )
{
    const struct region *r = find_exec( addr );

    if (!r) { printf( "0x%016lx  <not code>", addr ); return; }
    {
        const struct region *mod = module_of( r );

        if (mod)
        {
            unsigned long base = module_base( mod->path );
            const char *slash = strrchr( mod->path, '/' );

            printf( "0x%016lx  %s+0x%lx", addr, slash ? slash + 1 : mod->path, addr - base );
        }
        else printf( "0x%016lx  <anon>+0x%lx", addr, addr - r->start );
    }
}

static void describe( unsigned long addr )
{
    const struct region *r = find_exec( addr );

    if (!r) return;
    if (r->path[0])
    {
        unsigned long base = module_base( r->path );
        const char *slash = strrchr( r->path, '/' );
        printf( "    0x%016lx  %s+0x%lx\n", addr, slash ? slash + 1 : r->path, addr - base );
    }
    else printf( "    0x%016lx  <anonymous exec>\n", addr );
}

static int read_mem( int pid, unsigned long addr, void *buf, size_t len )
{
    struct iovec local = { buf, len };
    struct iovec remote = { (void *)addr, len };
    ssize_t got = process_vm_readv( pid, &local, 1, &remote, 1, 0 );

    return got == (ssize_t)len;
}

/* Wine enters unix code through __wine_syscall_dispatcher, which switches off
 * the PE stack.  The dispatcher leaves an iret-shaped record - RIP, CS,
 * EFLAGS, RSP, SS - in the syscall frame on the unix stack, so the PE context
 * can be recovered by looking for the two constant selectors. */
static int find_pe_context( int pid, unsigned long unix_sp, const struct region *ustack,
                            unsigned long *pe_rip, unsigned long *pe_rsp, unsigned long *frame )
{
    unsigned long addr, top = ustack->end;

    if (top - unix_sp > (64UL << 10)) top = unix_sp + (64UL << 10);
    for (addr = unix_sp & ~7UL; addr + 40 <= top; addr += 8)
    {
        unsigned long q[5];

        if (!read_mem( pid, addr, q, sizeof(q) )) continue;
        if (q[1] != 0x33 || q[4] != 0x2b) continue;      /* user CS / SS */
        if (!find_exec( q[0] )) continue;                /* RIP must be code */
        if (!find_any( q[3] )) continue;                 /* RSP must be mapped */
        *pe_rip = q[0];
        *pe_rsp = q[3];
        *frame = addr - 0x70;                            /* rip sits at +0x70 */
        return 1;
    }
    return 0;
}

/* A stack slot is only a real return address if the bytes before it decode as
 * a call.  Without this filter the scan is swamped by dead frames. */
static int is_return_address( int pid, unsigned long ret )
{
    unsigned char b[16];

    if (!read_mem( pid, ret - 16, b, sizeof(b) )) return 0;
    if (b[11] == 0xE8) return 1;                                     /* call rel32 */
    if (b[10] == 0xFF && b[11] == 0x15) return 1;                    /* call [rip+d32] */
    if (b[14] == 0xFF && b[15] >= 0xD0 && b[15] <= 0xD7) return 1;   /* call reg */
    if (b[13] == 0x41 && b[14] == 0xFF && b[15] >= 0xD0 && b[15] <= 0xD7) return 1;
    if (b[14] == 0xFF && b[15] >= 0x10 && b[15] <= 0x17) return 1;   /* call [reg] */
    if (b[13] == 0xFF && b[14] >= 0x50 && b[14] <= 0x57) return 1;   /* call [reg+d8] */
    /* call [reg+disp32] is 6 bytes, so it occupies b[10..15]. */
    if (b[10] == 0xFF && b[11] >= 0x90 && b[11] <= 0x97) return 1;
    return 0;
}

static void scan_thread( int pid, int tid )
{
    struct user_regs_struct regs;
    struct iovec iov = { &regs, sizeof(regs) };
    const struct region *ustack, *pstack;
    unsigned long pe_rip = 0, pe_rsp = 0, frame = 0, addr, top;
    int status, printed = 0;

    if (ptrace( PTRACE_SEIZE, tid, 0, 0 ) == -1) { printf( "=== TID %d === seize: %s\n", tid, strerror( errno ) ); return; }
    if (ptrace( PTRACE_INTERRUPT, tid, 0, 0 ) == -1) goto detach;
    if (waitpid( tid, &status, __WALL ) == -1) goto detach;
    if (ptrace( PTRACE_GETREGSET, tid, NT_PRSTATUS, &iov ) == -1) goto detach;

    printf( "=== TID %d ===\n", tid );
    if (!(ustack = find_any( regs.rsp ))) { printf( "  no unix stack\n" ); goto detach; }

    if (!find_pe_context( pid, regs.rsp, ustack, &pe_rip, &pe_rsp, &frame ))
    {
        printf( "  (no PE context - unix-only thread)  rip=" );
        describe_inline( regs.rip );
        printf( "\n" );
        goto detach;
    }

    printf( "  PE rip = " ); describe_inline( pe_rip );
    printf( "   PE rsp = 0x%lx\n", pe_rsp );
    {
        /* Callee-saved PE registers, as saved by the syscall dispatcher. These
         * are what a blocked thread was actually holding: RtlWaitOnAddress
         * keeps the waited-on address in r12, for instance, which is the only
         * reliable way to name the lock a thread is parked on. */
        unsigned long rbx = 0, r12 = 0, r13 = 0, r14 = 0, r15 = 0, rbp = 0;

        read_mem( pid, frame + 0x08, &rbx, sizeof(rbx) );
        read_mem( pid, frame + 0x50, &r12, sizeof(r12) );
        read_mem( pid, frame + 0x58, &r13, sizeof(r13) );
        read_mem( pid, frame + 0x60, &r14, sizeof(r14) );
        read_mem( pid, frame + 0x68, &r15, sizeof(r15) );
        read_mem( pid, frame + 0x98, &rbp, sizeof(rbp) );
        printf( "  regs rbx=0x%lx r12=0x%lx r13=0x%lx r14=0x%lx r15=0x%lx rbp=0x%lx\n",
                rbx, r12, r13, r14, r15, rbp );
    }

    if (!(pstack = find_any( pe_rsp ))) { printf( "  no PE stack region\n" ); goto detach; }
    top = pstack->end;
    if (top - pe_rsp > (256UL << 10)) top = pe_rsp + (256UL << 10);

    for (addr = pe_rsp & ~7UL; addr < top && printed < 40; addr += 8)
    {
        unsigned long value;

        if (!read_mem( pid, addr, &value, sizeof(value) )) continue;
        if (!find_exec( value )) continue;
        if (!is_return_address( pid, value )) continue;
        printf( "    0x%012lx  ", addr );
        describe_inline( value );
        printf( "\n" );
        printed++;
    }

detach:
    ptrace( PTRACE_DETACH, tid, 0, 0 );
}

int main( int argc, char **argv )
{
    int pid, i;

    if (argc < 2) { fprintf( stderr, "usage: %s <pid> [tid ...]\n", argv[0] ); return 2; }
    pid = atoi( argv[1] );
    load_regions( pid );

    if (argc > 2)
    {
        for (i = 2; i < argc; i++) scan_thread( pid, atoi( argv[i] ) );
        return 0;
    }
    {
        char dir[64];
        struct dirent *e;
        DIR *d;

        snprintf( dir, sizeof(dir), "/proc/%d/task", pid );
        if (!(d = opendir( dir ))) { perror( "task" ); return 1; }
        while ((e = readdir( d ))) if (e->d_name[0] != '.') scan_thread( pid, atoi( e->d_name ) );
        closedir( d );
    }
    return 0;
}
