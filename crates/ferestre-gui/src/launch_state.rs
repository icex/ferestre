//! Observe the game executable, independently of Xbox presence and network state.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub const TAG: &str = "FERESTRE_LAUNCH_ID";
static NEXT: AtomicU64 = AtomicU64::new(0);
pub fn tag() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn image(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn observed(root: &Path, uid: u32, tag: &str, executable: &str) -> bool {
    let expected = format!("{TAG}={tag}");
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        if entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .is_none()
        {
            return false;
        }
        let dir = entry.path();
        let Ok(status) = std::fs::read_to_string(dir.join("status")) else {
            return false;
        };
        let owner = status.lines().find_map(|s| {
            s.strip_prefix("Uid:")
                .and_then(|s| s.split_whitespace().next()?.parse::<u32>().ok())
        });
        if owner != Some(uid) {
            return false;
        }
        let Ok(env) = std::fs::read(dir.join("environ")) else {
            return false;
        };
        if !env.split(|b| *b == 0).any(|v| v == expected.as_bytes()) {
            return false;
        }
        let Ok(cmd) = std::fs::read(dir.join("cmdline")) else {
            return false;
        };
        let first = cmd.split(|b| *b == 0).next().unwrap_or_default();
        let name = String::from_utf8_lossy(first);
        image(&name).eq_ignore_ascii_case(image(executable))
    })
}
pub fn mark_ready(launches: &mut BTreeMap<String, stop::Launch>, key: &str, pgid: u32) -> bool {
    let Some(launch) = launches.get_mut(key).filter(|l| l.pgid == pgid && !l.ready) else {
        return false;
    };
    launch.ready = true;
    true
}
pub fn watch(
    ui: &Ui,
    key: &str,
    pgid: u32,
    tag: String,
    executable: String,
    finished: Arc<AtomicBool>,
) {
    let ui = ui.clone();
    let key = key.to_string();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            while !finished.load(Ordering::Acquire) {
                if observed(
                    Path::new("/proc"),
                    unsafe { libc::getuid() },
                    &tag,
                    &executable,
                ) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            false
        })
        .await;
        if matches!(result, Ok(true)) && mark_ready(&mut ui.running.borrow_mut(), &key, pgid) {
            render(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_is_bound_to_this_launch_and_resets_on_a_new_launch() {
        let mut launches = BTreeMap::from([(
            "GAME".into(),
            stop::Launch::new(42, std::time::Instant::now()),
        )]);
        assert!(!launches["GAME"].ready);
        assert!(!mark_ready(&mut launches, "GAME", 41));
        assert!(mark_ready(&mut launches, "GAME", 42));
        assert!(!mark_ready(&mut launches, "GAME", 42));
        launches.insert(
            "GAME".into(),
            stop::Launch::new(43, std::time::Instant::now()),
        );
        assert!(!mark_ready(&mut launches, "GAME", 42));
        launches.clear();
        assert!(!mark_ready(&mut launches, "GAME", 43));
    }
    #[test]
    fn only_the_expected_game_from_this_launch_and_user_counts() {
        let root = std::env::temp_dir().join(format!("ferestre-starting-{}", tag()));
        let dir = root.join("42");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("status"), "Uid:\t1000\t1000\t1000\t1000\n").unwrap();
        std::fs::write(dir.join("environ"), format!("{TAG}=ours\0OTHER=x\0")).unwrap();
        std::fs::write(dir.join("cmdline"), b"C:\\game\\Title.exe\0-arg\0").unwrap();
        assert!(observed(&root, 1000, "ours", "Binaries/title.EXE"));
        assert!(!observed(&root, 1001, "ours", "Title.exe"));
        assert!(!observed(&root, 1000, "other", "Title.exe"));
        assert!(!observed(&root, 1000, "ours", "Launcher.exe"));
        std::fs::write(dir.join("cmdline"), b"wineboot.exe\0Title.exe\0").unwrap();
        assert!(!observed(&root, 1000, "ours", "Title.exe"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
