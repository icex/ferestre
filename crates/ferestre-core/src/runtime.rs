//! Finding the installed patched Proton, and what it can be trusted to provide.
//!
//! Every runtime gap looks the same from outside: the title does not start.
//! Proton closing the inherited fds is exit code 1 with an empty log; a missing
//! Wine patch is "Critical Failure: GameInput Runtime could not be loaded" in a
//! log nobody thought to open. So the check happens before the launch: compare
//! what the recipe asks for against what the runtime provides, and refuse with
//! the name of the capability and the symptom its absence would have caused.
//!
//! A runtime build is expected to publish its own list -- `titles/SCHEMA.md`,
//! "a runtime build is expected to ship the list of what it provides". None
//! does yet, so there is a fallback that probes the two things visible from
//! outside a build: the `xgameruntime.dll` it installs, and the
//! `close_fds=False` change in the `proton` script. That fallback is a floor,
//! not an inventory. [`CapabilitySource`] says which kind of answer you got and
//! the explanation says so in words, because "missing" from a probe means
//! "could not be seen", not "known absent".

use crate::capability::{self, Capability, Match};
use crate::paths::Paths;
use crate::recipe::Recipe;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Where a runtime build publishes what it provides, relative to the
/// compatibility tool's root.
pub const MANIFEST_PATH: &str = "files/share/ferestre/capabilities.json";

/// The plain-text form `titles/SCHEMA.md` documents and `titles/validate.py
/// --runtime` already reads: one name per line, `#` comments. Accepted as well
/// as the JSON so a runtime build only has to write one of them. This is the
/// file [`Paths::runtime_capabilities_file`] names.
pub const LIST_PATH: &str = "ferestre-capabilities.txt";

/// The GDK implementation itself.
pub const XGAMERUNTIME_DLL: &str = "files/lib/wine/x86_64-windows/xgameruntime.dll";

/// The whole of `xgameruntime.dll` comes from one patch
/// (`patches/xgameruntime/0001-taskqueue-user-networking-gamesave-package.patch`),
/// so the file carries all five of these or is not our build at all. That is
/// why the probe can turn one file into five names without guessing per
/// function. It cannot tell a stale build from a current one, which is the
/// reason a published manifest is preferred over a probe.
const XGAMERUNTIME_CAPABILITIES: [&str; 5] = [
    "xgameruntime.gamesave",
    "xgameruntime.networking",
    "xgameruntime.package",
    "xgameruntime.taskqueue",
    "xgameruntime.user",
];

/// Proton's `run_proc()` closes inherited fds. The decrypted main image lives
/// in one, so without this change every GDK title exits 1.
const CLOSE_FDS_MARKER: &[u8] = b"close_fds=False";

/// What that change is half of. The other half is the Wine patch that maps the
/// image from `WINE_DLL_FILE_MAP`, and nothing on disk shows whether the Wine
/// build carries it -- so the probe reports this on the evidence of the Proton
/// half alone. It is the closest honest answer a probe has.
const MEMFD_CAPABILITY: &str = "loader.memfd-main-image";

/// How we learned what a runtime provides. It qualifies every answer derived
/// from it, so it travels with the answer rather than being logged and lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySource {
    /// The runtime's own JSON manifest.
    Manifest,
    /// The runtime's own plain-text list.
    List,
    /// The runtime published nothing; the set was probed from files on disk.
    Probed,
}

impl CapabilitySource {
    /// Whether the runtime told us, rather than us inferring it.
    pub fn is_published(self) -> bool {
        !matches!(self, CapabilitySource::Probed)
    }
}

/// A patched Proton on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntime {
    /// The compatibility tool's root -- the directory holding `proton`.
    pub path: PathBuf,
    /// From the `version` file, if it has one. Informational: nothing decides
    /// anything on a version, which is the point of capabilities.
    pub version: Option<String>,
    pub provides: BTreeSet<Capability>,
    pub source: CapabilitySource,
}

impl InstalledRuntime {
    /// Examine a specific directory. Fails if it is not a compatibility tool,
    /// naming it, because a wrong `XODUS_PROTON_DIR` should not read as "no
    /// runtime installed".
    pub fn at(dir: &Path) -> anyhow::Result<Self> {
        if !dir.is_dir() {
            anyhow::bail!("{}: no such runtime directory", dir.display());
        }
        if !dir.join("proton").is_file() {
            anyhow::bail!(
                "{}: not a Proton compatibility tool (no `proton` script)",
                dir.display()
            );
        }
        let (provides, source) = read_capabilities(dir)?;
        Ok(InstalledRuntime {
            path: dir.to_path_buf(),
            version: read_version(dir),
            provides,
            source,
        })
    }

    /// Examine the runtime [`Paths`] resolved.
    ///
    /// The search order lives there, with the rest of what `xodus-env.sh`
    /// resolves; a second copy of it here would be one to drift. A directory
    /// that came from `XODUS_PROTON_DIR` is used as given, so a wrong override
    /// is an error naming it rather than a silent fall-through to some other
    /// install the user did not choose.
    pub fn discover(paths: &Paths) -> anyhow::Result<Self> {
        let dir = paths.runtime_dir().ok_or_else(|| {
            anyhow::anyhow!(
                "no patched Proton runtime found. Install one (ferestre install-runtime), \
                 or set XODUS_PROTON_DIR to an existing compatibility tool"
            )
        })?;
        Self::at(dir)
    }

    pub fn provides(&self, capability: &str) -> bool {
        self.provides.contains(capability)
    }

    /// How to name this runtime in a message.
    pub fn label(&self) -> String {
        match &self.version {
            Some(v) => format!("{v} ({})", self.path.display()),
            None => format!("unversioned ({})", self.path.display()),
        }
    }
}

fn read_capabilities(dir: &Path) -> anyhow::Result<(BTreeSet<Capability>, CapabilitySource)> {
    let manifest = dir.join(MANIFEST_PATH);
    if manifest.is_file() {
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| anyhow::anyhow!("{}: {e}", manifest.display()))?;
        let names =
            parse_manifest(&text).map_err(|e| anyhow::anyhow!("{}: {e}", manifest.display()))?;
        return Ok((names, CapabilitySource::Manifest));
    }
    let list = dir.join(LIST_PATH);
    if list.is_file() {
        let text = std::fs::read_to_string(&list)
            .map_err(|e| anyhow::anyhow!("{}: {e}", list.display()))?;
        return Ok((parse_list(&text), CapabilitySource::List));
    }
    Ok((probe(dir), CapabilitySource::Probed))
}

/// A runtime's capability manifest: an array of names, an array of entries with
/// a `name`, an object mapping names to true/false, or any of those under a
/// `capabilities` (or `provides`) key.
///
/// A manifest that exists but does not parse is an error rather than a silent
/// fall back to probing: it is a bug in the runtime build, and hiding it would
/// produce exactly the mystery this module exists to prevent.
pub fn parse_manifest(text: &str) -> anyhow::Result<BTreeSet<Capability>> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    names_from(&value, true)
}

fn names_from(value: &serde_json::Value, unwrap: bool) -> anyhow::Result<BTreeSet<Capability>> {
    use serde_json::Value;
    let mut out = BTreeSet::new();
    let mut add = |name: &str| -> anyhow::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("empty capability name");
        }
        out.insert(name.to_string());
        Ok(())
    };
    match value {
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(s) => add(s)?,
                    Value::Object(entry) => {
                        let name = entry
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("an entry has no \"name\""))?;
                        add(name)?;
                    }
                    other => anyhow::bail!("expected a capability name, found {other}"),
                }
            }
        }
        Value::Object(map) => {
            if unwrap {
                for key in ["capabilities", "provides"] {
                    if let Some(inner) = map.get(key) {
                        return names_from(inner, false);
                    }
                }
            }
            // Otherwise the object is itself the list: {"gameinput.v2": true}.
            // false and null mean the build deliberately does not provide it,
            // which is worth honouring rather than reading only the keys.
            for (name, provided) in map {
                match provided {
                    Value::Bool(true) => add(name)?,
                    Value::Bool(false) | Value::Null => {}
                    other => anyhow::bail!("{name}: expected true or false, found {other}"),
                }
            }
        }
        other => anyhow::bail!("expected an array or object of capability names, found {other}"),
    }
    Ok(out)
}

/// One name per line, `#` comments -- the same reading as
/// `titles/validate.py --runtime`.
pub fn parse_list(text: &str) -> BTreeSet<Capability> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// What can be established about a runtime that publishes nothing.
///
/// Deliberately short. The rest of the registry -- the appmodel, gameinput,
/// winrt and winhttp patches -- lives inside a Wine build with no marker on
/// disk, and a probe that guessed at them would be worse than one that says it
/// does not know.
fn probe(dir: &Path) -> BTreeSet<Capability> {
    let mut out = BTreeSet::new();
    if proton_keeps_inherited_fds(dir) {
        out.insert(MEMFD_CAPABILITY.to_string());
    }
    if dir.join(XGAMERUNTIME_DLL).is_file() {
        out.extend(XGAMERUNTIME_CAPABILITIES.iter().map(|s| s.to_string()));
    }
    out
}

fn proton_keeps_inherited_fds(dir: &Path) -> bool {
    // Bytes, not text: the script is Python and only has to contain a marker,
    // so there is no reason to require it to be valid UTF-8.
    let Ok(bytes) = std::fs::read(dir.join("proton")) else {
        return false;
    };
    bytes
        .windows(CLOSE_FDS_MARKER.len())
        .any(|w| w == CLOSE_FDS_MARKER)
}

fn read_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("version")).ok()?;
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    // Proton writes "<build timestamp> <name>"; the name is the half a person
    // can compare against a release.
    let mut parts = line.splitn(2, char::is_whitespace);
    let stamp = parts.next().unwrap_or_default();
    match parts.next() {
        Some(name) if !stamp.is_empty() && stamp.chars().all(|c| c.is_ascii_digit()) => {
            Some(name.trim().to_string())
        }
        _ => Some(line.to_string()),
    }
}

/// One capability as `titles/capabilities.toml` describes it.
///
/// Unknown keys are allowed here, unlike in a recipe: the registry is the
/// runtime's own documentation and may grow a field, and a launcher that only
/// wants the symptom should not refuse to start because of one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CapabilityInfo {
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// What you see when the runtime does not have it. The field that turns a
    /// capability list into a diagnosis.
    #[serde(default)]
    pub symptom_if_missing: Option<String>,
    #[serde(default)]
    pub provided_by: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// WinRT runtime classes this capability makes activatable, and the dll
    /// that hosts each. Implementing a class is only half of it: a class the
    /// prefix has never been told about cannot be activated, however complete
    /// the code behind it. See [`crate::winrt`].
    #[serde(default, rename = "winrt-classes")]
    pub winrt_classes: Vec<WinrtClass>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WinrtClass {
    pub class: String,
    pub dll: String,
}

#[derive(Debug, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    capability: Vec<CapabilityInfo>,
}

/// `titles/capabilities.toml`, read only to say what a missing capability would
/// have looked like. Optional everywhere: without it the explanation still
/// names what is missing, it just cannot say what it would have cost.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    by_name: BTreeMap<String, CapabilityInfo>,
}

impl Registry {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let file: RegistryFile = toml::from_str(text)?;
        Ok(Registry {
            by_name: file
                .capability
                .into_iter()
                .map(|c| (c.name.clone(), c))
                .collect(),
        })
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// Every WinRT class the registry says the runtime can host, whether or not
    /// this build actually provides it. Deliberately not filtered by what the
    /// runtime advertises: registering a class whose dll is absent costs a
    /// registry key nobody reads, and *not* registering one that is present
    /// costs a title that cannot start.
    pub fn winrt_classes(&self) -> Vec<crate::winrt::Class> {
        self.by_name
            .values()
            .flat_map(|capability| capability.winrt_classes.iter())
            .map(|c| crate::winrt::Class::new(&c.class, &c.dll))
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&CapabilityInfo> {
        self.by_name.get(name)
    }

    pub fn symptom(&self, name: &str) -> Option<&str> {
        self.by_name
            .get(name)
            .and_then(|c| c.symptom_if_missing.as_deref())
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// The answer to "can this runtime run this title", with the words to say why
/// not.
#[derive(Debug, Clone)]
pub struct Assessment {
    pub matched: Match,
    /// How the runtime's capability set was obtained, so a caller can decide
    /// how much a refusal is worth. A probed miss is not proof of absence.
    pub source: CapabilitySource,
    pub explanation: String,
}

impl Assessment {
    pub fn is_satisfied(&self) -> bool {
        self.matched.is_satisfied()
    }
}

/// Check a recipe against a runtime. `registry` is optional; supply
/// `titles/capabilities.toml` and the explanation gains the symptom each
/// missing capability would have produced.
pub fn assess(
    recipe: &Recipe,
    runtime: &InstalledRuntime,
    registry: Option<&Registry>,
) -> Assessment {
    let matched = capability::check(
        recipe.runtime.requires.iter().map(String::as_str),
        recipe.runtime.wants.iter().map(String::as_str),
        &runtime.provides,
    );
    let explanation = explain(recipe, runtime, &matched, registry);
    Assessment {
        matched,
        source: runtime.source,
        explanation,
    }
}

fn explain(
    recipe: &Recipe,
    runtime: &InstalledRuntime,
    matched: &Match,
    registry: Option<&Registry>,
) -> String {
    let title = &recipe.title.name;
    let required = matched.missing_required.len();
    let wanted = matched.missing_wanted.len();
    let mut out = String::new();

    // A probe cannot see most of the registry, so a miss it reports is "not
    // found", not "not there". Saying "will not start" on that evidence would
    // be the same overclaim in the other direction as launching blind.
    let published = runtime.source.is_published();
    let verb = |n: usize| -> String {
        if published {
            format!("{} missing", plural(n, "is", "are"))
        } else {
            "could not be found".to_string()
        }
    };
    if required > 0 {
        let _ = writeln!(
            out,
            "{title} {} start on this runtime: {required} required {} {}.",
            if published { "will not" } else { "may not" },
            plural(required, "capability", "capabilities"),
            verb(required)
        );
    } else if wanted > 0 {
        let _ = writeln!(
            out,
            "{title} can start on this runtime, but {wanted} optional {} {}.",
            plural(wanted, "capability", "capabilities"),
            verb(wanted)
        );
    } else {
        let _ = writeln!(
            out,
            "{title}: this runtime provides everything the recipe asks for."
        );
    }
    let _ = writeln!(out, "  runtime: {}", runtime.label());

    // The heading agrees with the sentence above it: a probe found nothing,
    // which is not the same claim as the name being absent from the build.
    let word = if published { "missing" } else { "not found" };
    section(
        &mut out,
        &format!("required, {word}"),
        &matched.missing_required,
        registry,
    );
    section(
        &mut out,
        &format!("optional, {word}"),
        &matched.missing_wanted,
        registry,
    );

    if !runtime.source.is_published() {
        if required + wanted > 0 {
            let _ = write!(
                out,
                "\nThis runtime publishes no capability list, so what it provides was probed\n\
                 from the files on disk. A name above can be missing here and present in the\n\
                 build; a runtime should publish {MANIFEST_PATH}.\n"
            );
        } else {
            let _ = writeln!(
                out,
                "  (probed from the files on disk; this runtime publishes no capability list)"
            );
        }
    }
    out
}

fn section(
    out: &mut String,
    heading: &str,
    names: &BTreeSet<Capability>,
    registry: Option<&Registry>,
) {
    if names.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{heading}:");
    for name in names {
        let _ = writeln!(out, "  {name}");
        if let Some(symptom) = registry.and_then(|r| r.symptom(name)) {
            let _ = writeln!(out, "    symptom: {}", one_line(symptom));
        }
    }
}

/// The registry wraps prose across lines; a diagnosis reads better on one.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use std::sync::atomic::{AtomicU32, Ordering};

    // --- fixtures ------------------------------------------------------------

    const RECIPE: &str = r#"
schema = 1
[title]
product-id = "9NBLGGH2JHXJ"
name = "Example Title"
slug = "example"
[launch]
executable = 'Example.exe'
[runtime]
requires = ["loader.memfd-main-image", "gameinput.v2"]
wants = ["winhttp.websocket-close-timeout"]
[status]
state = "playable"
summary = "Runs."
"#;

    const REGISTRY: &str = r#"
schema = 1

[[capability]]
name = "gameinput.v2"
summary = "gameinput.dll exposes IGameInput_v2 and its dispatcher."
provided-by = ["patches/wine/0003-gameinput-v2-dispatcher-opt-in.patch"]
symptom-if-missing = "Critical Failure: GameInput Runtime could not be loaded"

[[capability]]
name = "winhttp.websocket-close-timeout"
summary = "WinHttp websockets accept the close timeout option."
symptom-if-missing = """
a websocket close waits for a peer that never answers, so shutdown or sign-out \
hangs"""
"#;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A directory that removes itself. Small enough not to be worth a
    /// dependency, and this crate has none for it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "ferestre-runtime-{}-{tag}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, content: &str) {
            let file = self.0.join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A compatibility tool as install-xodus-proton.sh leaves one.
    fn fake_runtime(tag: &str, patched: bool, with_dll: bool) -> TempDir {
        let dir = TempDir::new(tag);
        dir.write(
            "proton",
            if patched {
                "#!/usr/bin/env python3\nreturn subprocess.call(args, close_fds=False)\n"
            } else {
                "#!/usr/bin/env python3\nreturn subprocess.call(args)\n"
            },
        );
        dir.write(
            "version",
            "1788593799 xodus-bleeding-edge-11.0-20260803-3-g7c0b4354\n",
        );
        if with_dll {
            dir.write(XGAMERUNTIME_DLL, "MZ not really a dll");
        }
        dir
    }

    fn names(items: &[&str]) -> BTreeSet<Capability> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// A resolved environment whose runtime is this directory.
    fn paths_pointing_at(dir: &Path) -> Paths {
        Paths::resolve(&Env::from_map([
            ("HOME", "/home/tester"),
            ("XODUS_PROTON_DIR", &dir.display().to_string()),
        ]))
        .expect("should resolve")
    }

    // --- manifests -----------------------------------------------------------

    #[test]
    fn a_manifest_may_be_an_array_of_names() {
        let got = parse_manifest(r#"["loader.memfd-main-image", "gameinput.v2"]"#).unwrap();
        assert_eq!(got, names(&["loader.memfd-main-image", "gameinput.v2"]));
    }

    #[test]
    fn a_manifest_may_carry_entries_with_a_name() {
        // A build will want to publish more than the name eventually; reading
        // only the name means that does not become a breaking change.
        let got = parse_manifest(
            r#"[{"name": "gameinput.v2", "since": "11.0"}, {"name": "xgameruntime.user"}]"#,
        )
        .unwrap();
        assert_eq!(got, names(&["gameinput.v2", "xgameruntime.user"]));
    }

    #[test]
    fn a_manifest_may_be_an_object_and_false_means_not_provided() {
        let got = parse_manifest(r#"{"gameinput.v2": true, "media.real-video": false}"#).unwrap();
        assert_eq!(got, names(&["gameinput.v2"]));
    }

    #[test]
    fn a_manifest_may_wrap_the_list() {
        let got = parse_manifest(r#"{"schema": 1, "capabilities": ["gameinput.v2"]}"#).unwrap();
        assert_eq!(got, names(&["gameinput.v2"]));
    }

    #[test]
    fn a_broken_manifest_is_an_error_not_a_silent_fallback() {
        // Falling back to a probe here would report capabilities the build
        // does have as missing, which is the failure this module prevents.
        assert!(parse_manifest("{").is_err());
        assert!(parse_manifest("[42]").is_err());
        assert!(parse_manifest(r#"{"schema": 1}"#).is_err());
        assert!(parse_manifest(r#"[{"summary": "no name"}]"#).is_err());
    }

    #[test]
    fn a_plain_text_list_reads_like_the_validator_reads_it() {
        let got =
            parse_list("# what this build provides\ngameinput.v2\n\n  media.real-video  # kept\n");
        assert_eq!(got, names(&["gameinput.v2", "media.real-video"]));
    }

    // --- discovery -----------------------------------------------------------

    #[test]
    fn a_published_manifest_is_preferred_over_probing() {
        let dir = fake_runtime("manifest", true, true);
        dir.write(MANIFEST_PATH, r#"["gameinput.v2"]"#);
        let rt = InstalledRuntime::at(dir.path()).unwrap();
        assert_eq!(rt.source, CapabilitySource::Manifest);
        // Not the probe's answer: the build's own word, even where it is smaller.
        assert_eq!(rt.provides, names(&["gameinput.v2"]));
        assert!(!rt.provides("xgameruntime.user"));
    }

    #[test]
    fn a_plain_text_list_is_also_accepted() {
        let dir = fake_runtime("list", true, true);
        dir.write(LIST_PATH, "gameinput.v2\nxgameruntime.taskqueue\n");
        let rt = InstalledRuntime::at(dir.path()).unwrap();
        assert_eq!(rt.source, CapabilitySource::List);
        assert_eq!(
            rt.provides,
            names(&["gameinput.v2", "xgameruntime.taskqueue"])
        );
    }

    #[test]
    fn without_a_manifest_the_two_visible_things_are_probed() {
        let dir = fake_runtime("probe", true, true);
        let rt = InstalledRuntime::at(dir.path()).unwrap();
        assert_eq!(rt.source, CapabilitySource::Probed);
        assert!(
            rt.provides(MEMFD_CAPABILITY),
            "close_fds=False is in the script"
        );
        assert!(rt.provides("xgameruntime.taskqueue"));
        assert!(rt.provides("xgameruntime.gamesave"));
        // Nothing on disk shows these, so the probe must not claim them.
        assert!(!rt.provides("gameinput.v2"));
        assert!(!rt.provides("appmodel.package-identity"));
    }

    #[test]
    fn an_unpatched_proton_does_not_probe_as_keeping_inherited_fds() {
        // The one gap that costs every title: run_proc() closes the fd holding
        // the decrypted image and the title exits 1 with an empty log.
        let dir = fake_runtime("unpatched", false, false);
        let rt = InstalledRuntime::at(dir.path()).unwrap();
        assert!(!rt.provides(MEMFD_CAPABILITY));
        assert!(rt.provides.is_empty());
    }

    #[test]
    fn the_version_file_reads_as_the_build_name() {
        let dir = fake_runtime("version", true, false);
        let rt = InstalledRuntime::at(dir.path()).unwrap();
        assert_eq!(
            rt.version.as_deref(),
            Some("xodus-bleeding-edge-11.0-20260803-3-g7c0b4354")
        );
        assert!(rt.label().contains("xodus-bleeding-edge"));
    }

    #[test]
    fn a_directory_that_is_not_a_compat_tool_is_refused_by_name() {
        let dir = TempDir::new("empty");
        let err = InstalledRuntime::at(dir.path()).unwrap_err().to_string();
        assert!(err.contains("proton"), "should say what is missing: {err}");
        assert!(
            err.contains(&dir.path().display().to_string()),
            "should name the directory: {err}"
        );
    }

    #[test]
    fn discovery_examines_the_runtime_paths_resolved() {
        let dir = fake_runtime("discover", true, true);
        let rt = InstalledRuntime::discover(&paths_pointing_at(dir.path())).unwrap();
        assert_eq!(rt.path, dir.path());
        assert!(rt.provides(MEMFD_CAPABILITY));
    }

    #[test]
    fn a_wrong_runtime_override_is_an_error_naming_it() {
        // XODUS_PROTON_DIR is taken as given, so pointing it at the wrong place
        // must not read as "no runtime installed" -- and must not quietly launch
        // against some other install the user did not choose.
        let wrong = TempDir::new("wrong");
        let err = InstalledRuntime::discover(&paths_pointing_at(wrong.path()))
            .unwrap_err()
            .to_string();
        assert!(err.contains(&wrong.path().display().to_string()), "{err}");
    }

    #[test]
    fn discovery_with_nothing_installed_says_what_to_do() {
        let paths = Paths::resolve(&Env::from_map([("HOME", "/home/tester")])).unwrap();
        assert_eq!(paths.runtime_dir(), None);
        let err = InstalledRuntime::discover(&paths).unwrap_err().to_string();
        assert!(err.contains("XODUS_PROTON_DIR"), "{err}");
        assert!(err.contains("install-runtime"), "{err}");
    }

    // --- assessment ----------------------------------------------------------

    #[test]
    fn an_unsatisfiable_recipe_names_the_capability_and_its_symptom() {
        let recipe = Recipe::parse(RECIPE).unwrap();
        let registry = Registry::parse(REGISTRY).unwrap();
        // A runtime that publishes its list, and does not have the GameInput
        // patch: the one case where a refusal is a fact rather than a doubt.
        let dir = fake_runtime("gap", true, true);
        dir.write(MANIFEST_PATH, r#"["loader.memfd-main-image"]"#);
        let rt = InstalledRuntime::at(dir.path()).unwrap();

        let a = assess(&recipe, &rt, Some(&registry));
        assert!(!a.is_satisfied());
        assert_eq!(a.matched.missing_required, names(&["gameinput.v2"]));
        assert!(
            a.explanation.contains("1 required capability is missing"),
            "counted in English: {}",
            a.explanation
        );
        assert!(a.explanation.contains("gameinput.v2"), "{}", a.explanation);
        assert!(
            a.explanation
                .contains("GameInput Runtime could not be loaded"),
            "the symptom is the point of the message: {}",
            a.explanation
        );
        assert!(a.explanation.contains("Example Title"), "{}", a.explanation);
    }

    #[test]
    fn a_missing_optional_capability_is_reported_but_does_not_block() {
        let recipe = Recipe::parse(RECIPE).unwrap();
        let registry = Registry::parse(REGISTRY).unwrap();
        let dir = fake_runtime("optional", true, false);
        dir.write(
            MANIFEST_PATH,
            r#"["loader.memfd-main-image", "gameinput.v2"]"#,
        );
        let rt = InstalledRuntime::at(dir.path()).unwrap();

        let a = assess(&recipe, &rt, Some(&registry));
        assert!(a.is_satisfied());
        assert!(a.explanation.contains("can start"), "{}", a.explanation);
        assert!(
            a.explanation.contains("optional, missing"),
            "{}",
            a.explanation
        );
        assert!(
            a.explanation.contains("winhttp.websocket-close-timeout"),
            "{}",
            a.explanation
        );
        // Wrapped prose in the registry must not arrive as a broken line.
        assert!(
            a.explanation.contains("shutdown or sign-out hangs"),
            "{}",
            a.explanation
        );
    }

    #[test]
    fn a_probed_runtime_says_the_answer_was_probed() {
        // Otherwise a refusal reads as proof the build lacks something, when
        // all it proves is that the build does not say.
        let recipe = Recipe::parse(RECIPE).unwrap();
        let dir = fake_runtime("caveat", true, true);
        let rt = InstalledRuntime::at(dir.path()).unwrap();

        let a = assess(&recipe, &rt, None);
        assert_eq!(a.source, CapabilitySource::Probed);
        assert!(a.explanation.contains("probed"), "{}", a.explanation);
        // "may not", not "will not": the build may well have it and not say so.
        assert!(a.explanation.contains("may not start"), "{}", a.explanation);
        assert!(
            !a.explanation.contains("will not start"),
            "{}",
            a.explanation
        );
        assert!(a.explanation.contains(MANIFEST_PATH), "{}", a.explanation);
        // No registry: still names what is missing, just without the symptom.
        assert!(a.explanation.contains("gameinput.v2"), "{}", a.explanation);
        assert!(!a.explanation.contains("symptom:"), "{}", a.explanation);
    }

    #[test]
    fn a_runtime_that_provides_everything_says_so_plainly() {
        let recipe = Recipe::parse(RECIPE).unwrap();
        let dir = fake_runtime("complete", true, false);
        dir.write(
            MANIFEST_PATH,
            r#"["loader.memfd-main-image", "gameinput.v2", "winhttp.websocket-close-timeout"]"#,
        );
        let rt = InstalledRuntime::at(dir.path()).unwrap();

        let a = assess(&recipe, &rt, None);
        assert!(a.is_satisfied());
        assert!(a.matched.missing_wanted.is_empty());
        assert!(
            a.explanation.contains("provides everything"),
            "{}",
            a.explanation
        );
    }

    #[test]
    fn the_repositorys_capability_registry_parses() {
        // The registry is the vocabulary recipes are written in; if it and this
        // reader drift apart, every explanation silently loses its symptoms.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles/capabilities.toml");
        if !path.exists() {
            return; // packaged crate without the repository around it
        }
        let registry = Registry::load(&path).expect("registry should parse");
        assert!(!registry.is_empty());
        for name in XGAMERUNTIME_CAPABILITIES.iter().chain([&MEMFD_CAPABILITY]) {
            let entry = registry
                .get(name)
                .unwrap_or_else(|| panic!("{name} is probed for but not in capabilities.toml"));
            assert!(
                entry.symptom_if_missing.is_some(),
                "{name} has no symptom-if-missing, so a refusal cannot explain itself"
            );
        }
    }
}
