//! Whether Wine can find the host's SDL2, which PlayStation pads depend on.
//!
//! The proton shim takes hidraw away from DualShock 4 and DualSense pads so a
//! GDK title sees them as XInput gamepads. That leaves winebus's SDL bus as
//! their only route, and winebus dlopens `libSDL2-2.0.so.0` from the host: the
//! runtime does not ship one, and its evdev bus hands every pad but Steam's
//! virtual one to another backend. Without SDL2 the pad reaches nothing at all,
//! and nothing else says why.
//!
//! The search mirrors what that dlopen sees on a default launch: the runtime's
//! own `files/lib/x86_64-linux-gnu`, which Proton puts on the library path,
//! then the loader's cache. Not the inherited `LD_LIBRARY_PATH` -- the launch
//! removes it, so a library only reachable through it is not reachable then.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SDL2_SONAME: &str = "libSDL2-2.0.so.0";

pub const DEPENDENCY_HINT: &str = "Arch/CachyOS and Fedora: install sdl2-compat. Debian/Ubuntu: sudo apt install libsdl2-2.0-0. See docs/DEPENDENCIES.md";

/// Where the 64-bit `libSDL2-2.0.so.0` winebus would load comes from, or
/// `None` when it would find none. An unavailable diagnostic -- no `ldconfig`,
/// or no cache for it to read -- is an error, never evidence either way.
pub fn find_sdl2(runtime: &Path) -> Result<Option<PathBuf>> {
    let bundled = runtime.join("files/lib/x86_64-linux-gnu").join(SDL2_SONAME);
    if bundled.exists() {
        return Ok(Some(bundled));
    }
    let cache = ldconfig_cache()?;
    Ok(x86_64_entry(&cache, SDL2_SONAME))
}

/// `ldconfig -p`, from wherever the distribution keeps it. `/sbin` is often
/// not on an ordinary user's PATH.
fn ldconfig_cache() -> Result<String> {
    let mut last = None;
    for program in ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"] {
        match Command::new(program).arg("-p").env("LC_ALL", "C").output() {
            Ok(output) if output.status.success() => {
                return Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            Ok(_) => bail!("ldconfig could not read the library cache"),
            Err(error) => last = Some(error),
        }
    }
    Err(last.expect("at least one candidate was tried"))
        .context("looking for ldconfig to check the library cache")
}

/// The path the cache gives for a 64-bit x86 library. A 32-bit copy is listed
/// under the same soname and is no use to the 64-bit winebus.
fn x86_64_entry(cache: &str, soname: &str) -> Option<PathBuf> {
    cache.lines().find_map(|line| {
        let (name, rest) = line.trim().split_once(' ')?;
        let (flags, path) = rest.split_once(" => ")?;
        (name == soname && flags.contains("x86-64")).then(|| PathBuf::from(path.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CACHE: &str = "1234 libs found in cache `/etc/ld.so.cache'\n\
        \tlibSDL2_image-2.0.so.0 (libc6,x86-64) => /usr/lib/libSDL2_image-2.0.so.0\n\
        \tlibSDL2-2.0.so.0 (libc6) => /usr/lib32/libSDL2-2.0.so.0\n\
        \tlibSDL2-2.0.so.0 (libc6,x86-64) => /usr/lib/libSDL2-2.0.so.0\n\
        \tlibSDL2-2.0.so (libc6,x86-64) => /usr/lib/libSDL2-2.0.so\n";

    #[test]
    fn the_64_bit_library_is_found_and_neighbours_do_not_count() {
        assert_eq!(
            x86_64_entry(CACHE, SDL2_SONAME),
            Some(PathBuf::from("/usr/lib/libSDL2-2.0.so.0"))
        );
    }

    #[test]
    fn a_32_bit_or_development_copy_alone_is_missing() {
        let cache = "\tlibSDL2-2.0.so.0 (libc6) => /usr/lib32/libSDL2-2.0.so.0\n\
            \tlibSDL2-2.0.so (libc6,x86-64) => /usr/lib/libSDL2-2.0.so\n\
            \tlibSDL2_image-2.0.so.0 (libc6,x86-64) => /usr/lib/libSDL2_image-2.0.so.0\n";
        assert_eq!(x86_64_entry(cache, SDL2_SONAME), None);
        assert_eq!(x86_64_entry("", SDL2_SONAME), None);
    }

    #[test]
    fn a_copy_inside_the_runtime_is_used_first() {
        let runtime = tempfile::tempdir().unwrap();
        let lib = runtime.path().join("files/lib/x86_64-linux-gnu");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(SDL2_SONAME), b"").unwrap();
        assert_eq!(
            find_sdl2(runtime.path()).unwrap(),
            Some(lib.join(SDL2_SONAME))
        );
    }
}
