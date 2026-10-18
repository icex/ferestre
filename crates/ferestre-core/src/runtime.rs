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
use serde::{Deserialize, Serialize};
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
    capability::XGAMERUNTIME_GAMESAVE,
    capability::XGAMERUNTIME_NETWORKING,
    capability::XGAMERUNTIME_PACKAGE,
    capability::XGAMERUNTIME_TASKQUEUE,
    capability::XGAMERUNTIME_USER,
];

/// Proton's `run_proc()` closes inherited fds. The decrypted main image lives
/// in one, so without this change every GDK title exits 1.
const CLOSE_FDS_MARKER: &[u8] = b"close_fds=False";

/// What that change is half of. The other half is the Wine patch that maps the
/// image from `WINE_DLL_FILE_MAP`, and nothing on disk shows whether the Wine
/// build carries it -- so the probe reports this on the evidence of the Proton
/// half alone. It is the closest honest answer a probe has.
const MEMFD_CAPABILITY: &str = capability::MEMFD_MAIN_IMAGE;

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

    /// The runtime a launch would use when nothing more specific was chosen.
    ///
    /// A directory that came from `XODUS_PROTON_DIR` is used as given, so a
    /// wrong override is an error naming it rather than a silent fall-through
    /// to some other install the user did not choose. Otherwise it is the
    /// newest of [`all`]: the managed downloads in the state directory as well
    /// as the legacy Steam compatibility tool. An AppImage brings no runtime of
    /// its own, so looking only at the legacy places reported "no runtime" on
    /// exactly the machines `ferestre install-runtime` had set up.
    pub fn discover(paths: &Paths) -> anyhow::Result<Self> {
        if let Some(dir) = paths.runtime_override() {
            return Self::at(dir);
        }
        if let Some(runtime) = all(paths).into_iter().next() {
            return Ok(runtime);
        }
        if let Some(dir) = paths.runtime_dir() {
            // Found by probing but not usable: say what is wrong with it.
            return Self::at(dir);
        }
        anyhow::bail!(
            "no patched Proton runtime found. Install one (ferestre install-runtime), \
             or set XODUS_PROTON_DIR to an existing compatibility tool"
        )
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

/// A published runtime archive, found from the normal GitHub release feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub asset_name: String,
    pub url: String,
    /// The minisign signature published beside the archive
    /// (`<asset>.minisig`), if the release has one. A caller must not install
    /// an archive that has none; see [`verify_release_file`].
    pub signature_url: Option<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Select the newest runtime release in GitHub's reverse-chronological release
/// feed. It deliberately ignores launcher releases and checksum files.
pub fn latest_release(json: &str) -> anyhow::Result<Release> {
    let releases: Vec<GithubRelease> = serde_json::from_str(json)?;
    for release in releases {
        if !release.tag_name.starts_with("runtime-") {
            continue;
        }
        if let Some(asset) = release.assets.iter().find(|asset| {
            asset.name.starts_with("ferestre-runtime-") && asset.name.ends_with(".tar.xz")
        }) {
            let signature = format!("{}.minisig", asset.name);
            let signature_url = release
                .assets
                .iter()
                .find(|other| other.name == signature)
                .map(|other| other.browser_download_url.clone());
            return Ok(Release {
                tag: release.tag_name.clone(),
                asset_name: asset.name.clone(),
                url: asset.browser_download_url.clone(),
                signature_url,
            });
        }
    }
    anyhow::bail!("no runtime archive was published in the Ferestre releases")
}

/// Every valid runtime in a directory of versioned runtime installs. Invalid
/// entries are ignored: an interrupted download must not make all older,
/// working runtimes disappear from the selector.
pub fn installed(dir: &Path) -> Vec<InstalledRuntime> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut runtimes: Vec<_> = entries
        .flatten()
        .filter_map(|entry| InstalledRuntime::at(&entry.path()).ok())
        .collect();
    runtimes.sort_by(runtime_order);
    runtimes
}

/// Compare the numeric release, independent of the tool's branding. A newer
/// `ferestre-11.0.20261017.1` must precede `xodus-...20261013.3`, and build 10
/// must precede build 9. Keep the full name/path as deterministic tie breakers.
fn runtime_order(a: &InstalledRuntime, b: &InstalledRuntime) -> std::cmp::Ordering {
    let numbers = |runtime: &InstalledRuntime| -> Vec<u64> {
        runtime
            .version
            .as_deref()
            .unwrap_or_default()
            .split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().unwrap_or(u64::MAX))
            .collect()
    };
    numbers(b)
        .cmp(&numbers(a))
        .then_with(|| b.version.cmp(&a.version))
        .then_with(|| a.path.cmp(&b.path))
}

/// All usable runtimes known on this machine. A legacy configured runtime
/// remains visible alongside managed releases, which makes adopting Ferestre
/// non-destructive for an existing Steam compatibility-tool installation.
pub fn all(paths: &Paths) -> Vec<InstalledRuntime> {
    let mut runtimes = installed(&paths.runtimes_dir());
    if let Some(path) = paths.runtime_dir() {
        if let Ok(runtime) = InstalledRuntime::at(path) {
            if !runtimes.iter().any(|known| known.path == runtime.path) {
                runtimes.push(runtime);
            }
        }
    }
    runtimes.sort_by(runtime_order);
    runtimes
}

/// Per-title runtime choices kept outside shipped recipes. A release can update
/// the compatibility matrix without overwriting the runtime a person chose.
///
/// Unknown keys are tolerated: a file a newer build wrote must still give this
/// one the choices it can read, rather than read as no choices at all.
#[derive(Debug, Default, Deserialize, Serialize)]
struct Selections {
    #[serde(default)]
    titles: BTreeMap<String, PathBuf>,
}

fn selections_file(config_dir: &Path) -> PathBuf {
    config_dir.join("runtime-selections.toml")
}

/// The saved choices. `Ok(default)` when there is no file; an error naming
/// the file when there is one that cannot be read.
fn read_selections(config_dir: &Path) -> anyhow::Result<Selections> {
    let file = selections_file(config_dir);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Selections::default()),
        Err(e) => anyhow::bail!("{}: {e}", file.display()),
    };
    toml::from_str(&text).map_err(|e| {
        anyhow::anyhow!(
            "{}: {e}\nfix the file, or move it aside to forget every saved runtime choice",
            file.display()
        )
    })
}

/// Read a local runtime choice for a Store product id.
///
/// A file that cannot be read is said on stderr and treated as no choice, so
/// a launch still happens -- with the automatic runtime, which the message
/// says.
pub fn selected(config_dir: &Path, product_id: &str) -> Option<PathBuf> {
    match read_selections(config_dir) {
        Ok(selections) => selections
            .titles
            .get(&product_id.to_ascii_uppercase())
            .cloned(),
        Err(e) => {
            eprintln!("-- ignoring the saved runtime choices: {e}");
            None
        }
    }
}

/// Persist a local runtime choice. Passing `None` restores automatic selection.
///
/// A file that is there and cannot be read is an error, not a blank slate:
/// rewriting it with only this one choice would silently throw away every
/// other title's.
pub fn set_selected(
    config_dir: &Path,
    product_id: &str,
    path: Option<&Path>,
) -> anyhow::Result<()> {
    let file = selections_file(config_dir);
    let mut selections = read_selections(config_dir)?;
    match path {
        Some(path) => {
            selections
                .titles
                .insert(product_id.to_ascii_uppercase(), path.to_path_buf());
        }
        None => {
            selections.titles.remove(&product_id.to_ascii_uppercase());
        }
    }
    crate::cache::write(&file, toml::to_string_pretty(&selections)?.as_bytes())
}

/// The runtime a person asked for, for one title: their choice for that title
/// if they made one, otherwise the one `XODUS_PROTON_DIR` names.
///
/// `XODUS_PROTON_DIR` is a choice too, made for every title at once, so it is
/// the requested runtime rather than one more candidate that a newer managed
/// download would quietly outrank. A value that is not a runtime is an error
/// naming it, as [`InstalledRuntime::discover`] has always made it.
pub fn requested(paths: &Paths, product_id: &str) -> anyhow::Result<Option<PathBuf>> {
    if let Some(chosen) = selected(paths.config_dir(), product_id) {
        return Ok(Some(chosen));
    }
    match paths.runtime_override() {
        Some(dir) => InstalledRuntime::at(dir).map(|runtime| Some(runtime.path)),
        None => Ok(None),
    }
}

/// Pick the requested runtime, or the newest compatible installed runtime.
/// A path is only accepted when it names one of the discovered installs; this
/// prevents a stale title override from silently running an arbitrary Proton.
///
/// Refuses before anything else can look at the answer, which leaves a caller
/// no way to launch anyway or to explain the refusal. [`choose`] is the one to
/// use before a launch.
pub fn select<'a>(
    recipe: &Recipe,
    runtimes: &'a [InstalledRuntime],
    requested: Option<&Path>,
    registry: Option<&Registry>,
) -> anyhow::Result<&'a InstalledRuntime> {
    let mut candidates = runtimes
        .iter()
        .filter(|rt| assess(recipe, rt, registry).is_satisfied());
    if let Some(path) = requested {
        return candidates
            .find(|rt| same_path(&rt.path, path))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: selected runtime is unavailable or incompatible",
                    path.display()
                )
            });
    }
    candidates
        .next()
        .ok_or_else(|| anyhow::anyhow!("no installed runtime satisfies {}", recipe.title.name))
}

/// The runtime a launch would use, and what it is missing.
#[derive(Debug, Clone)]
pub struct Choice<'a> {
    pub runtime: &'a InstalledRuntime,
    /// Whether the runtime satisfies the recipe, with the words for why not.
    /// The decision whether to launch is the caller's: a probed runtime that
    /// "could not be seen" to have something may well have it, and `--force`
    /// exists for a person who knows better than the recipe.
    pub assessment: Assessment,
}

/// Pick the runtime for a launch without refusing on the recipe's behalf.
///
/// The requested runtime when there is one, whatever it is missing -- someone
/// chose it -- and an error only when it is not installed. Otherwise the
/// newest runtime that satisfies the recipe, and failing that the newest
/// runtime, so the caller can show what is missing, suggest an update, and
/// honour `--force`. An error only when there is no runtime at all.
pub fn choose<'a>(
    recipe: &Recipe,
    runtimes: &'a [InstalledRuntime],
    requested: Option<&Path>,
    registry: Option<&Registry>,
) -> anyhow::Result<Choice<'a>> {
    let choice = |runtime: &'a InstalledRuntime| Choice {
        runtime,
        assessment: assess(recipe, runtime, registry),
    };
    if let Some(path) = requested {
        return runtimes
            .iter()
            .find(|rt| same_path(&rt.path, path))
            .map(choice)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: the runtime chosen for {} is not installed; choose another, or \
                     clear the choice to use the newest",
                    path.display(),
                    recipe.title.name
                )
            });
    }
    let mut assessed = runtimes.iter().map(choice);
    let first = assessed.next().ok_or_else(|| {
        anyhow::anyhow!(
            "no patched Proton runtime is installed. Install one (ferestre install-runtime), \
             or set XODUS_PROTON_DIR to an existing compatibility tool"
        )
    })?;
    if first.assessment.is_satisfied() {
        return Ok(first);
    }
    Ok(assessed
        .find(|candidate| candidate.assessment.is_satisfied())
        .unwrap_or(first))
}

/// Two names for the same runtime: a choice saved through a symlink, or with
/// a trailing slash, still names the install it meant.
fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .is_some_and(|a| b.canonicalize().ok().is_some_and(|b| a == b))
}

/// The public half of the key release artefacts are signed with, in
/// minisign's format (the second line of a `.pub` file).
///
/// PLACEHOLDER: no real key is committed yet. Until this is replaced with the
/// release key, [`verify_release_file`] refuses everything, so a build made
/// from this tree cannot install a runtime it has not verified.
pub const RELEASE_PUBLIC_KEY: &str = "REPLACE-WITH-THE-MINISIGN-RELEASE-PUBLIC-KEY";

/// Check a downloaded release artefact against its minisign signature and the
/// key built into the launcher.
///
/// `signature` is the text of the `.minisig` file published beside the
/// artefact. The archive is read in chunks, so a 300 MB runtime is never held
/// in memory. A SHA256SUMS file from the same release would only say the
/// download matches what the release says; a signature says the release is
/// ours.
///
/// `expected_name` is the name of the asset that was asked for. A good
/// signature only says *some* file was signed with the release key, so the
/// signed (trusted) comment also has to name this one: `minisign -S` writes
/// `file:<name>` there by default, and release CI must keep that. Without the
/// check, whoever can swap a release's assets could serve another signed
/// file, such as an older runtime, under this one's name.
pub fn verify_release_file(
    path: &Path,
    signature: &str,
    expected_name: &str,
) -> anyhow::Result<()> {
    verify_file_with_key(RELEASE_PUBLIC_KEY, path, signature, expected_name)
}

/// [`verify_release_file`] with an explicit public key.
pub fn verify_file_with_key(
    public_key: &str,
    path: &Path,
    signature: &str,
    expected_name: &str,
) -> anyhow::Result<()> {
    use std::io::Read;

    let key = minisign_verify::PublicKey::from_base64(public_key.trim()).map_err(|e| {
        anyhow::anyhow!(
            "this build carries no usable release key ({e}), so nothing it downloads can be \
             verified"
        )
    })?;
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|e| anyhow::anyhow!("the signature for {} is unreadable: {e}", path.display()))?;
    let mut verifier = key.verify_stream(&signature).map_err(|e| {
        anyhow::anyhow!("{} is not signed with the release key: {e}", path.display())
    })?;
    let mut file =
        std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier.finalize().map_err(|e| {
        anyhow::anyhow!(
            "{} does not match its signature ({e}); it was not installed",
            path.display()
        )
    })?;
    // Only read now that the global signature has vouched for it.
    let names_it = signature
        .trusted_comment()
        .split('\t')
        .any(|field| field.strip_prefix("file:") == Some(expected_name));
    if expected_name.is_empty() || !names_it {
        anyhow::bail!(
            "{} is signed, but as a different file (\"{}\") from the {expected_name} that was \
             asked for; it was not installed",
            path.display(),
            signature.trusted_comment()
        );
    }
    Ok(())
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

    #[test]
    fn release_discovery_ignores_launcher_assets_and_finds_the_runtime_archive() {
        let release = latest_release(
            r#"[
              {"tag_name":"v0.1.2","assets":[{"name":"ferestre-0.1.2.AppImage","browser_download_url":"https://example.invalid/gui"}]},
              {"tag_name":"runtime-11.0.20260908","assets":[
                {"name":"SHA256SUMS","browser_download_url":"https://example.invalid/sums"},
                {"name":"ferestre-runtime-11.0.20260908.tar.xz","browser_download_url":"https://example.invalid/runtime"}
              ]}
            ]"#,
        )
        .unwrap();
        assert_eq!(release.tag, "runtime-11.0.20260908");
        assert_eq!(release.url, "https://example.invalid/runtime");
    }

    #[test]
    fn runtime_inventory_keeps_valid_versions_and_skips_partial_downloads() {
        let root = TempDir::new("runtime-inventory");
        let older = root.path().join("11.0.1");
        let newer = root.path().join("11.0.2");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::create_dir_all(&newer).unwrap();
        std::fs::write(older.join("proton"), "#!/bin/sh\nclose_fds=False\n").unwrap();
        std::fs::write(newer.join("proton"), "#!/bin/sh\nclose_fds=False\n").unwrap();
        std::fs::write(
            older.join("version"),
            "0 xodus-bleeding-edge-11.0.20261017.9\n",
        )
        .unwrap();
        std::fs::write(newer.join("version"), "0 ferestre-11.0.20261017.10\n").unwrap();
        std::fs::create_dir_all(root.path().join("partial")).unwrap();

        let found = installed(root.path());
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].version.as_deref(),
            Some("ferestre-11.0.20261017.10")
        );
        assert_eq!(
            found[1].version.as_deref(),
            Some("xodus-bleeding-edge-11.0.20261017.9")
        );
    }

    #[test]
    fn a_title_runtime_choice_round_trips_without_touching_the_recipe() {
        let config = TempDir::new("runtime-choice");
        let chosen = Path::new("/opt/ferestre/runtimes/11.0.2");
        set_selected(config.path(), "9nnx1vvr3knq", Some(chosen)).unwrap();
        assert_eq!(
            selected(config.path(), "9NNX1VVR3KNQ").as_deref(),
            Some(chosen)
        );
        set_selected(config.path(), "9NNX1VVR3KNQ", None).unwrap();
        assert_eq!(selected(config.path(), "9NNX1VVR3KNQ"), None);
    }

    #[test]
    fn selection_prefers_the_newest_compatible_runtime_and_honours_an_override() {
        let old = fake_runtime("select-old", true, false);
        let new = fake_runtime("select-new", true, false);
        old.write("version", "11.0.1\n");
        new.write("version", "11.0.2\n");
        old.write(
            MANIFEST_PATH,
            r#"["loader.memfd-main-image", "gameinput.v2"]"#,
        );
        new.write(
            MANIFEST_PATH,
            r#"["loader.memfd-main-image", "gameinput.v2"]"#,
        );
        let runtimes = vec![
            InstalledRuntime::at(new.path()).unwrap(),
            InstalledRuntime::at(old.path()).unwrap(),
        ];
        let recipe = Recipe::parse(RECIPE).unwrap();
        assert_eq!(
            select(&recipe, &runtimes, None, None).unwrap().path,
            new.path()
        );
        assert_eq!(
            select(&recipe, &runtimes, Some(old.path()), None)
                .unwrap()
                .path,
            old.path()
        );
    }

    #[test]
    fn a_saved_choice_file_that_cannot_be_read_is_not_overwritten() {
        // Rewriting it with only the new choice would throw away every other
        // title's without a word.
        let config = TempDir::new("runtime-choice-corrupt");
        let file = config.path().join("runtime-selections.toml");
        std::fs::write(&file, "[titles\nbroken = ").unwrap();
        let chosen = Path::new("/opt/ferestre/runtimes/11.0.2");
        let err = set_selected(config.path(), "9NNX1VVR3KNQ", Some(chosen))
            .unwrap_err()
            .to_string();
        assert!(err.contains("runtime-selections.toml"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[titles\nbroken = "
        );
        assert_eq!(selected(config.path(), "9NNX1VVR3KNQ"), None);

        // A key from a newer build is no reason to lose the choices.
        std::fs::write(
            &file,
            "future = 1\n[titles]\n9NNX1VVR3KNQ = \"/opt/a\"\n9PPT8K6GQHRZ = \"/opt/b\"\n",
        )
        .unwrap();
        assert_eq!(
            selected(config.path(), "9PPT8K6GQHRZ").as_deref(),
            Some(Path::new("/opt/b"))
        );
        set_selected(config.path(), "9NNX1VVR3KNQ", Some(chosen)).unwrap();
        assert_eq!(
            selected(config.path(), "9PPT8K6GQHRZ").as_deref(),
            Some(Path::new("/opt/b"))
        );
        assert_eq!(
            selected(config.path(), "9NNX1VVR3KNQ").as_deref(),
            Some(chosen)
        );
    }

    /// The gate after the choice decides whether to launch; the choice itself
    /// must hand it a runtime and the reasons, not refuse first.
    #[test]
    fn choosing_leaves_the_refusal_to_the_caller() {
        let probed = fake_runtime("choose-probed", true, true);
        probed.write("version", "11.0.3\n");
        let lacking = fake_runtime("choose-lacking", true, false);
        lacking.write("version", "11.0.2\n");
        lacking.write(MANIFEST_PATH, r#"["loader.memfd-main-image"]"#);
        let complete = fake_runtime("choose-complete", true, false);
        complete.write("version", "11.0.1\n");
        complete.write(
            MANIFEST_PATH,
            r#"["loader.memfd-main-image", "gameinput.v2"]"#,
        );
        let recipe = Recipe::parse(RECIPE).unwrap();
        let mut runtimes = vec![
            InstalledRuntime::at(probed.path()).unwrap(),
            InstalledRuntime::at(lacking.path()).unwrap(),
            InstalledRuntime::at(complete.path()).unwrap(),
        ];
        runtimes.sort_by(runtime_order);

        // The newest one that satisfies the recipe, even if older.
        let choice = choose(&recipe, &runtimes, None, None).unwrap();
        assert_eq!(choice.runtime.path, complete.path());
        assert!(choice.assessment.is_satisfied());

        // A probed runtime that was asked for is handed over with its doubts,
        // where select() refused it outright.
        assert!(select(&recipe, &runtimes, Some(probed.path()), None).is_err());
        let choice = choose(&recipe, &runtimes, Some(probed.path()), None).unwrap();
        assert_eq!(choice.runtime.path, probed.path());
        assert_eq!(choice.assessment.source, CapabilitySource::Probed);
        assert!(!choice.assessment.is_satisfied());

        // Nothing satisfies it: the newest, with what it is missing, so the
        // caller can explain it and honour --force.
        let only_lacking = vec![InstalledRuntime::at(lacking.path()).unwrap()];
        assert!(select(&recipe, &only_lacking, None, None).is_err());
        let choice = choose(&recipe, &only_lacking, None, None).unwrap();
        assert_eq!(choice.runtime.path, lacking.path());
        assert!(choice
            .assessment
            .matched
            .missing_required
            .contains("gameinput.v2"));

        // A choice that is not installed, and no runtime at all, are errors.
        assert!(choose(&recipe, &runtimes, Some(Path::new("/nonexistent")), None).is_err());
        let err = choose(&recipe, &[], None, None).unwrap_err().to_string();
        assert!(err.contains("install-runtime"), "{err}");
    }

    /// A managed runtime in the state directory, which is all an AppImage
    /// user has.
    fn managed_paths(state: &Path, extra: &[(&str, &str)]) -> Paths {
        let mut vars = vec![
            ("HOME", "/home/tester".to_string()),
            ("XDG_STATE_HOME", state.display().to_string()),
            (
                "XDG_CONFIG_HOME",
                state.join("config").display().to_string(),
            ),
        ];
        vars.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        Paths::resolve(&Env::from_map(vars)).unwrap()
    }

    #[test]
    fn discovery_finds_a_runtime_install_runtime_put_in_the_state_directory() {
        let state = TempDir::new("managed-state");
        let managed = state.path().join("ferestre/runtimes/11.0.5");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join("proton"), "close_fds=False\n").unwrap();
        let paths = managed_paths(state.path(), &[]);
        assert_eq!(paths.runtime_dir(), None);
        let found = InstalledRuntime::discover(&paths).expect("the managed runtime");
        assert_eq!(found.path, managed);
    }

    #[test]
    fn the_runtime_override_is_the_requested_runtime() {
        let state = TempDir::new("override-state");
        let managed = state.path().join("ferestre/runtimes/99.0.0");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join("proton"), "close_fds=False\n").unwrap();
        std::fs::write(managed.join("version"), "99.0.0\n").unwrap();
        let named = fake_runtime("override-named", true, true);
        named.write("version", "1.0.0\n");
        let paths = managed_paths(
            state.path(),
            &[("XODUS_PROTON_DIR", &named.path().display().to_string())],
        );

        // Newer managed downloads do not outrank the one that was named.
        assert_eq!(
            InstalledRuntime::discover(&paths).unwrap().path,
            named.path()
        );
        assert_eq!(
            requested(&paths, "9NBLGGH2JHXJ").unwrap().as_deref(),
            Some(named.path())
        );
        let runtimes = all(&paths);
        let recipe = Recipe::parse(RECIPE).unwrap();
        let wanted = requested(&paths, "9NBLGGH2JHXJ").unwrap();
        let choice = choose(&recipe, &runtimes, wanted.as_deref(), None).unwrap();
        assert_eq!(choice.runtime.path, named.path());

        // A per-title choice is more specific than the variable.
        let config = paths.config_dir().to_path_buf();
        set_selected(&config, "9NBLGGH2JHXJ", Some(&managed)).unwrap();
        assert_eq!(
            requested(&paths, "9NBLGGH2JHXJ").unwrap().as_deref(),
            Some(managed.as_path())
        );
        std::fs::remove_dir_all(&config).unwrap();

        // A wrong value is an error naming it, not a silent fall back.
        let wrong = TempDir::new("override-wrong");
        let paths = managed_paths(
            state.path(),
            &[("XODUS_PROTON_DIR", &wrong.path().display().to_string())],
        );
        let err = requested(&paths, "9NBLGGH2JHXJ").unwrap_err().to_string();
        assert!(err.contains(&wrong.path().display().to_string()), "{err}");
        assert!(InstalledRuntime::discover(&paths).is_err());
    }

    #[test]
    fn a_runtime_release_carries_its_signature() {
        let release = latest_release(
            r#"[{"tag_name":"runtime-11.0.20261018","assets":[
                {"name":"ferestre-runtime-11.0.20261018.tar.xz","browser_download_url":"https://example.invalid/runtime"},
                {"name":"ferestre-runtime-11.0.20261018.tar.xz.minisig","browser_download_url":"https://example.invalid/runtime.minisig"}
            ]}]"#,
        )
        .unwrap();
        assert_eq!(
            release.signature_url.as_deref(),
            Some("https://example.invalid/runtime.minisig")
        );
        let unsigned = latest_release(
            r#"[{"tag_name":"runtime-1","assets":[
                {"name":"ferestre-runtime-1.tar.xz","browser_download_url":"https://example.invalid/r"}]}]"#,
        )
        .unwrap();
        assert_eq!(unsigned.signature_url, None);
    }

    /// A minisign key pair made here, for this test only, and a signer that
    /// writes what `minisign -S` writes: a BLAKE2b-512 prehash signed with
    /// Ed25519, and a global signature over that and the trusted comment.
    struct TestSigner {
        key: ed25519_dalek::SigningKey,
        key_id: [u8; 8],
    }

    impl TestSigner {
        fn generate() -> TestSigner {
            use sha2::Digest;
            let mut seed_source = sha2::Sha256::new();
            seed_source.update(std::process::id().to_le_bytes());
            seed_source.update(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
                    .to_le_bytes(),
            );
            let seed: [u8; 32] = seed_source.finalize().into();
            let mut key_id = [0u8; 8];
            key_id.copy_from_slice(&seed[..8]);
            TestSigner {
                key: ed25519_dalek::SigningKey::from_bytes(&seed),
                key_id,
            }
        }

        fn public_key(&self) -> String {
            let mut bytes = b"Ed".to_vec();
            bytes.extend_from_slice(&self.key_id);
            bytes.extend_from_slice(self.key.verifying_key().as_bytes());
            base64(&bytes)
        }

        /// Signed as `minisign -S -m <name>` signs, with the default
        /// trusted comment naming the file.
        fn sign(&self, data: &[u8], name: &str) -> String {
            use ed25519_dalek::Signer;
            let signature = self.key.sign(&blake2b512(data)).to_bytes();
            let mut line = b"ED".to_vec();
            line.extend_from_slice(&self.key_id);
            line.extend_from_slice(&signature);
            let trusted = format!("timestamp:0\tfile:{name}\thashed");
            let mut global = signature.to_vec();
            global.extend_from_slice(trusted.as_bytes());
            let global = self.key.sign(&global).to_bytes();
            format!(
                "untrusted comment: test signature\n{}\ntrusted comment: {trusted}\n{}\n",
                base64(&line),
                base64(&global)
            )
        }
    }

    fn base64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// BLAKE2b with a 64-byte digest and no key (RFC 7693), which is what
    /// minisign prehashes with. Only for signing in this test.
    fn blake2b512(data: &[u8]) -> [u8; 64] {
        const IV: [u64; 8] = [
            0x6a09e667f3bcc908,
            0xbb67ae8584caa73b,
            0x3c6ef372fe94f82b,
            0xa54ff53a5f1d36f1,
            0x510e527fade682d1,
            0x9b05688c2b3e6c1f,
            0x1f83d9abfb41bd6b,
            0x5be0cd19137e2179,
        ];
        const SIGMA: [[usize; 16]; 10] = [
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
            [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
            [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
            [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
            [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
            [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
            [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
            [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
            [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
        ];
        fn compress(h: &mut [u64; 8], block: &[u8; 128], counter: u128, last: bool) {
            let mut m = [0u64; 16];
            for (i, word) in m.iter_mut().enumerate() {
                *word = u64::from_le_bytes(block[i * 8..i * 8 + 8].try_into().unwrap());
            }
            let mut v = [0u64; 16];
            v[..8].copy_from_slice(h);
            v[8..].copy_from_slice(&IV);
            v[12] ^= counter as u64;
            v[13] ^= (counter >> 64) as u64;
            if last {
                v[14] = !v[14];
            }
            let mut g = |a: usize, b: usize, c: usize, d: usize, x: u64, y: u64| {
                v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
                v[d] = (v[d] ^ v[a]).rotate_right(32);
                v[c] = v[c].wrapping_add(v[d]);
                v[b] = (v[b] ^ v[c]).rotate_right(24);
                v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
                v[d] = (v[d] ^ v[a]).rotate_right(16);
                v[c] = v[c].wrapping_add(v[d]);
                v[b] = (v[b] ^ v[c]).rotate_right(63);
            };
            for round in 0..12 {
                let s = &SIGMA[round % 10];
                g(0, 4, 8, 12, m[s[0]], m[s[1]]);
                g(1, 5, 9, 13, m[s[2]], m[s[3]]);
                g(2, 6, 10, 14, m[s[4]], m[s[5]]);
                g(3, 7, 11, 15, m[s[6]], m[s[7]]);
                g(0, 5, 10, 15, m[s[8]], m[s[9]]);
                g(1, 6, 11, 12, m[s[10]], m[s[11]]);
                g(2, 7, 8, 13, m[s[12]], m[s[13]]);
                g(3, 4, 9, 14, m[s[14]], m[s[15]]);
            }
            for i in 0..8 {
                h[i] ^= v[i] ^ v[i + 8];
            }
        }
        let mut h = IV;
        h[0] ^= 0x0101_0000 ^ 64;
        let mut counter: u128 = 0;
        let mut blocks = data.chunks(128).peekable();
        if blocks.peek().is_none() {
            compress(&mut h, &[0u8; 128], 0, true);
        }
        while let Some(chunk) = blocks.next() {
            let mut block = [0u8; 128];
            block[..chunk.len()].copy_from_slice(chunk);
            counter += chunk.len() as u128;
            compress(&mut h, &block, counter, blocks.peek().is_none());
        }
        let mut out = [0u8; 64];
        for (i, word) in h.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    #[test]
    fn the_test_hash_is_blake2b() {
        // RFC 7693, appendix A.
        let hex: String = blake2b512(b"abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1\
             7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
    }

    #[test]
    fn a_release_artefact_is_installed_only_with_a_good_signature() {
        const NAME: &str = "ferestre-runtime-test.tar.xz";
        let dir = TempDir::new("signature");
        let archive = dir.path().join(NAME);
        // Larger than one read, so the streaming path is what is checked.
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&archive, &data).unwrap();

        let signer = TestSigner::generate();
        let key = signer.public_key();
        let signature = signer.sign(&data, NAME);
        verify_file_with_key(&key, &archive, &signature, NAME).expect("a good signature verifies");

        // One byte changed.
        let mut tampered = data.clone();
        tampered[123_456] ^= 1;
        std::fs::write(&archive, &tampered).unwrap();
        let err = verify_file_with_key(&key, &archive, &signature, NAME)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match"), "{err}");
        std::fs::write(&archive, &data).unwrap();

        // Signed by somebody else's key that claims the release key's id, so
        // what refuses it is the Ed25519 check, not the id comparison.
        let mut other = TestSigner::generate();
        assert_ne!(
            other.key.verifying_key(),
            signer.key.verifying_key(),
            "two different keys"
        );
        other.key_id = signer.key_id;
        let err = verify_file_with_key(&key, &archive, &other.sign(&data, NAME), NAME)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match"), "{err}");
        // Not a signature at all.
        assert!(verify_file_with_key(&key, &archive, "garbage", NAME).is_err());

        // A genuine signature, for another file of the release: an older
        // runtime served under this one's name.
        let older = signer.sign(&data, "ferestre-runtime-older.tar.xz");
        let err = verify_file_with_key(&key, &archive, &older, NAME)
            .unwrap_err()
            .to_string();
        assert!(err.contains("different file"), "{err}");
        // A name that only starts like the expected one is not it.
        let longer = signer.sign(&data, &format!("{NAME}.old"));
        assert!(verify_file_with_key(&key, &archive, &longer, NAME).is_err());
        // Editing the comment to name the right file breaks the global
        // signature, so the name cannot be relabelled after signing.
        let relabelled = older.replace(
            "file:ferestre-runtime-older.tar.xz",
            &format!("file:{NAME}"),
        );
        assert_ne!(relabelled, older);
        let err = verify_file_with_key(&key, &archive, &relabelled, NAME)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match"), "{err}");
        assert!(verify_file_with_key(&key, &archive, &signature, "").is_err());

        // The placeholder key verifies nothing, so a build that still has it
        // cannot install an archive it did not check.
        assert!(verify_release_file(&archive, &signature, NAME).is_err());
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
