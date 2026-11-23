//! A CPU layout Wine can use on a machine whose online CPUs are numbered with
//! gaps (some cores offline, say CPUs 0-1 and 4-15).
//!
//! Wine sizes its processor table by the number of online CPUs and then fills
//! it by host CPU number, so on such a machine a thread on CPU 14 of 14 online
//! CPUs gets an id past the table's end. `WINE_CPU_TOPOLOGY` gives Wine a dense
//! mapping instead, and the game is kept to the CPUs in it. A machine numbered
//! without gaps, or a launch that sets `WINE_CPU_TOPOLOGY` itself, is left
//! alone.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

pub const ENV: &str = "WINE_CPU_TOPOLOGY";

/// The mapping to give Wine and the CPUs the game may run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// The value of `WINE_CPU_TOPOLOGY`.
    pub env: String,
    pub cpus: Vec<usize>,
}

/// A Linux CPU list such as `0-1,4-15`.
pub fn parse_list(text: &str) -> Result<BTreeSet<usize>> {
    let mut cpus = BTreeSet::new();
    for item in text.trim().split(',').filter(|i| !i.is_empty()) {
        let (first, last) = match item.split_once('-') {
            Some((a, b)) => (a.parse::<usize>()?, b.parse::<usize>()?),
            None => {
                let cpu = item.parse::<usize>()?;
                (cpu, cpu)
            }
        };
        if last < first {
            bail!("{text:?}: not a CPU list");
        }
        cpus.extend(first..=last);
    }
    Ok(cpus)
}

/// The mapping for `online` CPUs the launch may use (`affinity`), with each
/// CPU's SMT siblings from `siblings`. `None` when the numbering has no gaps.
pub fn mapping(
    online: &BTreeSet<usize>,
    affinity: &BTreeSet<usize>,
    siblings: impl Fn(usize) -> Option<BTreeSet<usize>>,
) -> Result<Option<Mapping>> {
    if online.iter().copied().eq(0..online.len()) {
        return Ok(None);
    }
    // Wine bounds host ids by the online count (and its mask by 64).
    let limit = online.len().min(64);
    let mut remaining: BTreeSet<usize> = online
        .intersection(affinity)
        .copied()
        .filter(|cpu| *cpu < limit)
        .collect();
    if remaining.is_empty() {
        bail!(
            "no CPU this launch may use is one Wine can number on this machine; \
             set {ENV} yourself, or start it with another CPU affinity"
        );
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    while let Some(&cpu) = remaining.iter().next() {
        let group: Vec<usize> = siblings(cpu)
            .unwrap_or_else(|| BTreeSet::from([cpu]))
            .intersection(&remaining)
            .copied()
            .collect();
        let group = if group.contains(&cpu) {
            group
        } else {
            vec![cpu]
        };
        for c in &group {
            remaining.remove(c);
        }
        groups.push(group);
    }
    // The syntax says two threads per core, or one: two only when every
    // core kept both.
    let smt = groups.iter().all(|g| g.len() == 2);
    let cpus: Vec<usize> = groups.iter().flatten().copied().collect();
    let count = if smt {
        format!("{}s", groups.len())
    } else {
        cpus.len().to_string()
    };
    let list: Vec<String> = cpus.iter().map(usize::to_string).collect();
    Ok(Some(Mapping {
        env: format!("{count}:{}", list.join(",")),
        cpus,
    }))
}

/// The mapping for this machine and this process's affinity.
pub fn for_host(cpu_root: &Path) -> Result<Option<Mapping>> {
    let online = std::fs::read_to_string(cpu_root.join("online"))
        .with_context(|| format!("{}", cpu_root.join("online").display()))?;
    let online = parse_list(&online)?;
    let affinity = affinity().unwrap_or_else(|| online.clone());
    mapping(&online, &affinity, |cpu| {
        std::fs::read_to_string(cpu_root.join(format!("cpu{cpu}/topology/thread_siblings_list")))
            .ok()
            .and_then(|text| parse_list(&text).ok())
    })
}

fn affinity() -> Option<BTreeSet<usize>> {
    // SAFETY: a zeroed cpu_set_t is a valid empty set, filled in by the call.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return None;
        }
        Some(
            (0..libc::CPU_SETSIZE as usize)
                .filter(|cpu| libc::CPU_ISSET(*cpu, &set))
                .collect(),
        )
    }
}

/// Keep the calling process (a child between fork and exec) to `cpus`.
/// Async-signal-safe: no allocation.
pub fn restrict_to(cpus: &[usize]) -> std::io::Result<()> {
    // SAFETY: as in `affinity`; the set is filled in before the call reads it.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for cpu in cpus {
            libc::CPU_SET(*cpu, &mut set);
        }
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(text: &str) -> BTreeSet<usize> {
        parse_list(text).unwrap()
    }

    /// Pairs (0,1), (2,3), … as SMT siblings.
    fn pairs(cpu: usize) -> Option<BTreeSet<usize>> {
        Some(BTreeSet::from([cpu & !1, cpu | 1]))
    }

    #[test]
    fn cpu_lists_parse_and_bad_ones_are_refused() {
        assert_eq!(set("0-1,4,6-7\n"), BTreeSet::from([0, 1, 4, 6, 7]));
        assert!(parse_list("3-1").is_err());
        assert!(parse_list("a").is_err());
    }

    #[test]
    fn a_machine_numbered_without_gaps_is_left_alone() {
        assert_eq!(mapping(&set("0-15"), &set("0-15"), pairs).unwrap(), None);
    }

    #[test]
    fn gaps_map_to_the_cpus_wine_can_number_keeping_smt_pairs() {
        // 14 online, numbered 0-1 and 4-15: Wine can number ids below 14.
        let m = mapping(&set("0-1,4-15"), &set("0-1,4-15"), pairs)
            .unwrap()
            .unwrap();
        assert_eq!(m.env, "6s:0,1,4,5,6,7,8,9,10,11,12,13");
        assert_eq!(m.cpus, vec![0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    }

    #[test]
    fn a_core_missing_a_thread_makes_every_cpu_its_own_core() {
        let m = mapping(&set("0-1,4-15"), &set("0-1,4-12"), pairs)
            .unwrap()
            .unwrap();
        assert_eq!(m.env, "11:0,1,4,5,6,7,8,9,10,11,12");
        let none = mapping(&set("0-1,4-15"), &set("14-15"), pairs);
        assert!(none.unwrap_err().to_string().contains(ENV));
    }
}
