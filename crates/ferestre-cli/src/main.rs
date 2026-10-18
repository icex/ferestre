//! The `ferestre` command.
//!
//! This replaces `packaging/appimage/ferestre-placeholder.sh`, so it keeps that
//! script's subcommand names and its output shape: people have already run it,
//! and a rewrite that renames everything is a rewrite that breaks their
//! shortcuts.
//!
//! It owns exactly two things -- starting child processes and printing -- and
//! deliberately nothing else. Where things live, which recipe applies, what the
//! runtime provides and what environment a title needs are all decided in
//! `ferestre-core`, so the GUI that comes next behaves identically without
//! reimplementing any of it. When you find yourself about to make a decision in
//! this file, it belongs in the other crate.
//!
//! Two rules run through the whole thing:
//!
//!   * `--json` puts the document on stdout and nothing else. Progress,
//!     warnings and errors go to stderr, so `ferestre library --json | jq` works
//!     with no flags.
//!   * No account identifier is printed unless `--show-account` is given. Any
//!     output here must be safe to paste into a bug report unedited, which is
//!     the rule `tools/xsts_probe.py` already follows.

mod cancel;

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use ferestre_core::accounts;
use ferestre_core::autofix;
use ferestre_core::capability;
use ferestre_core::catalog;
use ferestre_core::http;
use ferestre_core::install;
use ferestre_core::launch;
use ferestre_core::library;
use ferestre_core::paths::Paths;
use ferestre_core::recipe::Recipe;
use ferestre_core::runtime::{self, Assessment, InstalledRuntime, Registry};
use ferestre_core::winrt;
use serde::Serialize;
use serde_json::{json, Value};

/// `2` is also what clap exits with on a bad command line, so a usage failure
/// looks the same whether clap or we caught it.
const EXIT_FAILURE: u8 = 1;
const EXIT_USAGE: u8 = 2;

/// Version of the `--json` documents this build emits. A GUI built against 1
/// should refuse 2 rather than misread it.
const JSON_SCHEMA: u32 = 1;

// ---------------------------------------------------------------------------
// command line
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "ferestre",
    version,
    about = "Run the Microsoft Store and Xbox GDK titles you own",
    long_about = "Downloads and runs the Microsoft Store / Xbox GDK titles you own, through \
                  the Xodus client and a patched Proton runtime.\n\n\
                  Paths come from XODUS_* environment variables; `ferestre env` shows what they \
                  resolve to. Product ids are the 12-character code in a Store URL."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,

    /// Print a JSON document instead of text. stdout carries only the document.
    #[arg(long, global = true)]
    json: bool,

    /// Directory of title recipes. Defaults to the packaged `titles/`.
    #[arg(long, value_name = "DIR", global = true)]
    titles_dir: Option<PathBuf>,

    /// Print account identifiers (XUID, gamertag) instead of masking them.
    #[arg(long, global = true)]
    show_account: bool,

    /// Never colour the output. `NO_COLOR` in the environment does the same.
    #[arg(long, global = true)]
    no_color: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Check this machine for everything a launch needs.
    Doctor,

    /// List the titles that have a launch recipe.
    Titles,

    /// List what the signed-in account owns.
    Library {
        /// Include the rows normally filtered out, with the reason they were.
        #[arg(long)]
        all: bool,
        /// Ignore any cached listing. Accepted and harmless: nothing is cached
        /// yet, so every listing is already live.
        #[arg(long)]
        refresh: bool,
        /// Answer from the cache without touching the network.
        #[arg(long)]
        offline: bool,
    },

    /// Launch a title by product id or slug.
    Run {
        /// Product id or slug, as `ferestre titles` lists them.
        #[arg(value_name = "TITLE")]
        title: String,
        /// Print the resolved command and environment instead of launching.
        #[arg(long)]
        dry_run: bool,
        /// Launch even though the runtime is missing a required capability.
        #[arg(long)]
        force: bool,
        /// Keep the environment Steam started us with, for a non-Steam shortcut
        /// that wants the overlay and Remote Play. Not needed for its Steam
        /// Input controller layout, which works without it.
        #[arg(long)]
        steam: bool,
        /// Arguments for the title. Not supported yet; see the error.
        #[arg(last = true, value_name = "ARG")]
        args: Vec<String>,
    },

    /// Download and decrypt a title you own.
    #[command(alias = "download")]
    Install {
        /// The 12-character Store product id.
        #[arg(value_name = "PRODUCT-ID")]
        product_id: String,
        /// Where to put it. Defaults to the recipe's install directory.
        #[arg(value_name = "DIR")]
        dir: Option<PathBuf>,
    },

    /// Check installed MSIXVC packages against the authenticated update service.
    Updates,

    /// Remove an installed title: its files and the launcher's record of it.
    #[command(alias = "remove")]
    Uninstall {
        /// The 12-character Store product id.
        #[arg(value_name = "PRODUCT-ID")]
        product_id: String,
        /// Forget the record but leave the files where they are.
        #[arg(long)]
        keep_files: bool,
        /// Also remove the title's Wine prefix. Saved games usually live there,
        /// so it is kept unless this is passed.
        #[arg(long)]
        prefix: bool,
        /// Do not ask. Required when there is no terminal to ask at.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Build and install the patched Proton runtime.
    InstallRuntime,

    /// Inspect an Appx or Msix archive without extracting or launching it.
    InspectPackage {
        /// The .appx, .msix, .appxbundle, or .msixbundle archive.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },

    /// Print the resolved XODUS_* paths.
    Env,

    /// Print the version.
    Version,
}

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(&cli) {
        Ok(code) => code,
        Err(err) => {
            print_error(&err);
            ExitCode::from(exit_code_for(&err))
        }
    }
}

/// Failures are reported the way the shell scripts report one: `!!` and the
/// message, then the chain of causes indented under it. anyhow's own Display
/// prints only the outermost error, which is usually the least informative.
fn print_error(err: &anyhow::Error) {
    eprintln!("!! {err}");
    for cause in err.chain().skip(1) {
        eprintln!("   {cause}");
    }
}

fn exit_code_for(err: &anyhow::Error) -> u8 {
    if err.downcast_ref::<UsageError>().is_some() {
        EXIT_USAGE
    } else {
        EXIT_FAILURE
    }
}

fn dispatch(cli: &Cli) -> Result<ExitCode> {
    match &cli.command {
        Cmd::Doctor => cmd_doctor(cli),
        Cmd::Titles => cmd_titles(cli),
        Cmd::Library {
            all,
            refresh,
            offline,
        } => cmd_library(cli, *all, *refresh, *offline),
        Cmd::Run {
            title,
            dry_run,
            force,
            steam,
            args,
        } => cmd_run(cli, title, *dry_run, *force, *steam, args),
        Cmd::Install { product_id, dir } => cmd_install(cli, product_id, dir.as_deref()),
        Cmd::Updates => cmd_updates(cli),
        Cmd::Uninstall {
            product_id,
            keep_files,
            prefix,
            yes,
        } => cmd_uninstall(cli, product_id, *keep_files, *prefix, *yes),
        Cmd::InstallRuntime => cmd_install_runtime(cli),
        Cmd::InspectPackage { path } => cmd_inspect_package(cli, path),
        Cmd::Env => cmd_env(cli),
        Cmd::Version => cmd_version(cli),
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

fn cmd_inspect_package(cli: &Cli, path: &Path) -> Result<ExitCode> {
    let package = ferestre_core::appx::inspect_archive(path)?;
    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "path": path.display().to_string(),
            "package": package,
        }));
    } else {
        let identity = package.identity.as_deref().unwrap_or("unknown identity");
        let version = package.version.as_deref().unwrap_or("unknown version");
        println!("{identity} {version}");
        println!(
            "  executable: {}",
            package.executable.as_deref().unwrap_or("not declared")
        );
        println!(
            "  entry point: {}",
            package.entry_point.as_deref().unwrap_or("not declared")
        );
        println!(
            "  model: {}",
            match package.kind {
                ferestre_core::appx::PackageKind::DesktopBridge =>
                    "desktop bridge (candidate; launch support is not implemented yet)",
                ferestre_core::appx::PackageKind::Uwp => "UWP (requires app-model support)",
                ferestre_core::appx::PackageKind::Unknown => "unknown",
            }
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_version(cli: &Cli) -> Result<ExitCode> {
    let packaged = std::env::var("FERESTRE_PACKAGE_VERSION").ok();
    let version = ferestre_core::package_version(packaged.as_deref());
    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "name": "ferestre",
            "version": version,
        }));
    } else {
        println!("{version}");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_doctor(cli: &Cli) -> Result<ExitCode> {
    // Not `?`: doctor is the command you run *because* resolution failed, so it
    // has to report that rather than exit on it.
    let checks = match Paths::from_env() {
        Ok(paths) => doctor_checks(&paths),
        Err(err) => {
            vec![Check::fail("host", "cannot resolve any paths").with_hint(err.to_string())]
        }
    };
    let failed = failures(&checks);

    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "ready": failed == 0,
            "failures": failed,
            "checks": checks,
        }));
    } else {
        let style = Style::detect(std::io::stdout().is_terminal(), cli.no_color);
        print!("{}", render_checks(&checks, &style));
        println!();
        if failed == 0 {
            println!("{}ready.{}  next: ferestre titles", style.ok, style.off);
        } else {
            println!(
                "{}{failed} blocking problem(s) above.{}",
                style.bad, style.off
            );
        }
    }

    // The exit status is the answer, not decoration: a script can gate on it.
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILURE)
    })
}

fn cmd_titles(cli: &Cli) -> Result<ExitCode> {
    let (dir, recipes) = load_recipes(cli)?;

    if cli.json {
        let rows: Vec<TitleRow> = recipes.iter().map(TitleRow::from).collect();
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "titles_dir": dir.display().to_string(),
            "titles": rows,
        }));
        return Ok(ExitCode::SUCCESS);
    }

    if recipes.is_empty() {
        // Not an error: an empty recipe directory is a legitimate state, and a
        // caller tells it apart by the empty list rather than by an exit code.
        println!("no recipes in {}", dir.display());
        return Ok(ExitCode::SUCCESS);
    }

    println!("recipes in {}:\n", dir.display());
    print!("{}", indent(&table(&titles_table(&recipes)), "  "));
    println!("\nlaunch one with: ferestre run <product-id|slug>");
    Ok(ExitCode::SUCCESS)
}

fn cmd_library(cli: &Cli, all: bool, refresh: bool, offline: bool) -> Result<ExitCode> {
    if offline {
        // Said plainly rather than silently served from nothing: the cache in
        // docs/CATALOG.md §8 is designed, not written.
        return Err(usage(
            "--offline needs a cached listing, and nothing is cached yet",
        ));
    }
    if refresh {
        eprintln!("-- --refresh is a no-op: nothing is cached yet, so every listing is live");
    }

    let paths = Paths::from_env()?;
    let xodus_cli = paths
        .xodus_cli()
        .ok_or_else(|| anyhow!("no xodus-cli found; set XODUS_CLI_DIR (see: ferestre doctor)"))?;
    let (_, recipes) = load_recipes(cli)?;

    // The client's stdout is the document and its stderr is progress, so only
    // stdout is captured. Shelling out is not an implementation detail here: it
    // is the licence boundary (see NOTICE) and it is checked by the fact that
    // this crate has no Xodus dependency to link.
    let mut command = library::library_command(&xodus_cli, &library::Query::default());
    if let Some(roles) = accounts::load(paths.config_dir())? {
        roles.apply(&mut command);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());
    let output = command
        .output()
        .with_context(|| format!("running {}", xodus_cli.display()))?;
    if !output.status.success() {
        bail!(
            "{} library failed (exit {}); if this is a sign-in problem, run: {} login",
            xodus_cli.display(),
            output.status.code().unwrap_or(-1),
            xodus_cli.display()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let entries = library::parse(&text)?;
    let skipped: Vec<(String, &'static str)> = library::skipped(&entries)
        .into_iter()
        .map(|(e, why)| (e.product_id.clone(), why))
        .collect();
    let rows = library::join(&entries, &recipes);
    let shown: Vec<&library::Row> = rows
        .iter()
        .filter(|r| all || r.entry.is_some_and(library::Entry::is_title))
        .collect();

    if cli.json {
        let mut doc = library_json(&shown, &skipped);
        // Last thing before stdout. The document is not supposed to carry an
        // account identifier at all; the cost of being wrong about that is a
        // XUID in a pasted bug report, so it is masked here as well.
        redact_json(&mut doc, cli.show_account);
        print_json(&doc);
        return Ok(ExitCode::SUCCESS);
    }

    if shown.is_empty() {
        println!("nothing owned that this launcher can run (try --all)");
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", indent(&table(&library_table(&shown)), "  "));
    if !all && !skipped.is_empty() {
        println!(
            "\n{} row(s) filtered out; --all shows them with the reason",
            skipped.len()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn update_command(xodus_cli: &Path, content_id: &str) -> Command {
    let mut command = Command::new(xodus_cli);
    command.arg("update").arg(content_id).arg("--json");
    command
}

/// The client's streaming command is also its update delivery command.
///
/// It compares the package's segment metadata with the tree already in
/// `destination`, keeps matching files, and downloads only changed segments.
/// Keeping this construction in one place means the GUI's Update action and
/// `ferestre install PRODUCT` share the same safe, resumable path.
fn streaming_command(xodus_cli: &Path, product_id: &str, destination: &Path) -> Command {
    let mut command = Command::new(xodus_cli);
    command.arg("streaming").arg(product_id).arg(destination);
    command
}

fn cmd_updates(cli: &Cli) -> Result<ExitCode> {
    let paths = Paths::from_env()?;
    let xodus_cli = paths
        .xodus_cli()
        .ok_or_else(|| anyhow!("no xodus-cli found; set XODUS_CLI_DIR (see: ferestre doctor)"))?;
    let records: Vec<_> = install::all(paths.state_dir())
        .into_iter()
        .filter(|record| !record.is_stale())
        .collect();
    let mut results = Vec::new();
    for record in records {
        let Some(content_id) = record.content_ids.first() else {
            continue;
        };
        let mut command = update_command(&xodus_cli, content_id);
        if let Some(roles) = accounts::load(paths.config_dir())? {
            roles.apply(&mut command);
        }
        let output = command
            .output()
            .with_context(|| format!("running {} update", xodus_cli.display()))?;
        if !output.status.success() {
            eprintln!("-- {}: update service unavailable", record.product_id);
            continue;
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .context("the update client returned invalid JSON")?;
        let version = value["version"].as_str().unwrap_or_default().to_string();
        let available = record.update_available(Some(&version));
        results.push(json!({
            "product_id": record.product_id,
            "installed_version": record.package_version,
            "available_version": version,
            "update_available": available,
        }));
    }
    if cli.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "updates": results }));
    } else if results.is_empty() {
        println!("no installed packages could be checked");
    } else {
        for result in results {
            let state = if result["update_available"] == Value::Bool(true) {
                "update available"
            } else {
                "up to date"
            };
            println!(
                "{}: {}",
                result["product_id"].as_str().unwrap_or("package"),
                state
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_run(
    cli: &Cli,
    title: &str,
    dry_run: bool,
    force: bool,
    steam: bool,
    args: &[String],
) -> Result<ExitCode> {
    if !args.is_empty() {
        // Better than passing them somewhere they will be ignored: `xodus-cli
        // run` takes the executable and the game directory, and has nowhere to
        // put a title's own arguments.
        return Err(usage(
            "passing arguments to a title is not supported yet: the client's run command takes none",
        ));
    }

    let paths = Paths::from_env()?;
    let (dir, mut recipes) = load_recipes(cli)?;

    // Nothing describes it, but it is on disk and its package says how to start
    // it -- so describe it and carry on rather than stopping to ask. The
    // manifest is the authority here: it names the entry point, which is the
    // one thing a person would otherwise have to go and find in a tree of
    // thousands of files. Writing it down means the answer survives, and stays
    // editable when a title turns out to need something else.
    if !recipes.iter().any(|r| r.matches(title)) {
        if let Some(written) = describe_from_disk(&paths, title)? {
            eprintln!(
                "-- no recipe, so one was written from the package manifest: {}",
                written.display()
            );
            recipes = load_recipes(cli)?.1;
        }
    }

    let recipe = recipes.iter().find(|r| r.matches(title)).ok_or_else(|| {
        anyhow!(
            "no recipe for '{title}' in {} (see: ferestre titles)",
            dir.display()
        )
    })?;

    // A title without a tested recipe can still share an engine with one that
    // has been debugged. The same inference feeds the window's verdict.
    let inferred = autofix::infer(recipe, Some(&paths.install_dir(recipe)?));

    // Checked before anything is started, because the alternative is a title
    // that exits 1 with an empty log and someone guessing which of eleven
    // patches is missing.
    let runtimes = runtime::all(&paths);
    let requested_runtime = runtime::selected(paths.config_dir(), &recipe.title.product_id);
    let runtime = runtime::select(
        &inferred.recipe,
        &runtimes,
        requested_runtime.as_deref(),
        registry_for(&paths).as_ref(),
    )?;
    let recipe = &engine_defaults(inferred, runtime, &paths);
    let registry = registry_for(&paths);
    let assessment = runtime::assess(recipe, runtime, registry.as_ref());
    let verdict = gate(&assessment);
    if verdict == Gate::Refused && !force {
        bail!("{}", refusal(&assessment, runtime));
    }
    for line in warnings(recipe, &assessment, verdict, force) {
        eprintln!("{line}");
    }

    let dirs = launch_dirs_for_runtime(&paths, &runtime.path)?;
    let roles = accounts::launch_selection(paths.config_dir())?;
    let mut prefix = paths.prefix_dir(recipe)?;
    // `XGR_XUID` is the Windows install's XUID, so it names the imported
    // player's save folders. Another player has their own prefix and their own
    // XUID: inheriting it would name their saves after somebody else, and
    // report that XUID whenever the live lookup has not answered yet.
    let mut own_xuid = true;
    if let Some(roles) = &roles {
        let profiles = accounts::fetch(&dirs.xodus_cli())?;
        let (store, player) = roles.selected(&profiles)?;
        if roles.split() && !runtime.provides(accounts::CAPABILITY) {
            bail!("This runtime cannot separate Store and Player identities. Select an account-sharing runtime, or clear the account role selections.");
        }
        if !runtime.provides(SERVICE_SOCKET_OVERRIDE) {
            bail!("This runtime cannot isolate the selected account service. Update the runtime first.");
        }
        accounts::require_service(&dirs.xodus_service())?;
        prefix = accounts::player_prefix(&prefix, player)?;
        own_xuid = player.legacy_saves;
        if cli.show_account {
            eprintln!(":: Store: {}; Player: {}", store.label, player.label);
        } else {
            eprintln!(":: using the selected Store and Player accounts");
        }
    }
    if let Some(registry) = &registry {
        register_winrt_classes(&prefix, registry, runtime);
    }
    let options = launch::Options {
        steam_integration: steam,
        xuid: paths
            .var("XGR_XUID")
            .filter(|_| own_xuid)
            .map(str::to_string),
        game_dir: Some(paths.install_dir(recipe)?),
        prefix_dir: Some(prefix),
    };
    let mut plan = launch::plan(recipe, &dirs, &options);
    if !own_xuid {
        plan.env_set.remove("XGR_XUID");
        plan.env_unset.push("XGR_XUID".into());
    }
    if let Some(roles) = &roles {
        plan.env_set
            .insert("XODUS_ACCOUNT_PROFILE".into(), roles.store.clone());
        plan.env_set
            .insert("XODUS_STORE_PROFILE".into(), roles.store.clone());
        plan.env_set
            .insert("XODUS_PLAYER_PROFILE".into(), roles.player.clone());
        if runtime.provides(accounts::CAPABILITY) {
            plan.env_set
                .insert("XGR_ACCOUNT_ROLES_V1".into(), "1".into());
        }
        let record = install::all(paths.state_dir()).into_iter()
            .find(|r| r.product_id.eq_ignore_ascii_case(&recipe.title.product_id))
            .context("Account sharing needs an install record with its authenticated content ID. Refresh or reinstall this title.")?;
        let content = record
            .content_ids
            .first()
            .context("The installed title has no Store content ID")?;
        // Upper case like every Store id the runtime sends; the service
        // compares the bound product exactly.
        plan.env_set.insert(
            "XODUS_STORE_PRODUCT".into(),
            recipe.title.product_id.to_ascii_uppercase(),
        );
        plan.env_set
            .insert("XODUS_STORE_CONTENT".into(), content.clone());
    }
    let service_socket = plan_service_socket(&mut plan, &dirs, runtime, roles.as_ref())?;

    if dry_run {
        if cli.json {
            print_json(&json!({ "schema": JSON_SCHEMA, "plan": plan_json(&plan) }));
        } else {
            print!("{}", render_plan(&plan));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // The three things a Plan deliberately does not model, in the order
    // launch-gdk.sh does them.
    let game_dir = launch::game_dir(recipe, &dirs, &options);
    if !game_dir.is_dir() {
        bail!(
            "{} is not installed at {} -- download it: ferestre install {}",
            recipe.title.name,
            game_dir.display(),
            recipe.title.product_id
        );
    }
    run_setup(recipe, "every-launch", &dirs, &game_dir)?;
    std::fs::create_dir_all(launch::prefix_dir(recipe, &dirs, &options))
        .context("creating the Wine prefix directory")?;
    if runtime.provides("xgameruntime.playfab-file-saves") {
        if let Some(identity) = install::package_identity(&game_dir) {
            let backups = paths
                .state_dir()
                .join("save-backups")
                .join(&recipe.title.product_id);
            for backup in ferestre_core::playfab::repair_package(
                &launch::prefix_dir(recipe, &dirs, &options),
                &identity,
                &backups,
            )? {
                eprintln!(
                    ":: preserved unsynced save cache at {}; saved progress will be rescanned",
                    backup.display()
                );
            }
        }
    }
    // From here on a signal must not end this process outright: it holds a
    // claim on the service, released only by finishing.
    let forwarding = Forwarding::install()?;
    // Before the service is looked for: a launch finishing now must not stop a
    // service this one is about to reuse.
    let _service_use = ServiceUse::begin(service_socket.as_deref())?;
    start_service(&dirs, service_socket.as_deref(), paths.cache_dir(), &plan)?;
    if let Some(signal) = forwarding.received() {
        eprintln!("-- stopped before {} started", recipe.title.name);
        return Ok(ExitCode::from(exit_code_from(None, Some(signal))));
    }

    if let Ok(missing) = ferestre_core::media::missing_libav_dependencies(
        &dirs.proton_dir,
        Some(&dirs.steam_dir),
        ferestre_core::media::launch_library_path(&plan).as_deref(),
    ) {
        if !missing.is_empty() {
            eprintln!(
                "-- videos may be black or silent: missing {}. {}",
                missing.join(", "),
                ferestre_core::media::DEPENDENCY_HINT
            );
        }
    }
    run_teed(
        plan.to_command(),
        &launch::log_path(recipe, &dirs),
        &forwarding,
    )
}

/// The recipe a launch uses once a detected engine's defaults are settled,
/// with one line saying what happened to them.
///
/// Said whatever the outcome, short of no engine at all: someone comparing a
/// run with and without a flag needs to know whether the launcher added it, and
/// someone who opted out needs to see that the opt-out was honoured.
fn engine_defaults(
    inferred: autofix::Inferred,
    runtime: &InstalledRuntime,
    paths: &Paths,
) -> Recipe {
    let mut recipe = inferred.recipe.into_owned();
    let Some(profile) = inferred.profile else {
        return recipe;
    };
    let (key, token) = profile.setting();
    // Raw, not `var`: an empty value is the opt-out, not an absence.
    let outcome = autofix::apply(
        &mut recipe,
        Some(profile),
        &runtime.provides,
        paths.var_raw(key),
    );
    let (engine, effect) = (profile.engine(), profile.effect());
    match outcome {
        autofix::Outcome::NotDetected => {}
        autofix::Outcome::Applied => {
            eprintln!("-- detected {engine}: enabled {effect} ({key}={token})")
        }
        autofix::Outcome::Merged { previous } => {
            eprintln!(
                "-- detected {engine}: enabled {effect} by adding {token} to {key}={previous}"
            )
        }
        autofix::Outcome::AlreadyPresent => {
            eprintln!("-- detected {engine}: {key} already enables {effect}")
        }
        autofix::Outcome::SkippedExplicitEmpty => {
            eprintln!("-- detected {engine}: {key} is set and empty, so {effect} stays off")
        }
        autofix::Outcome::RuntimeLacks => eprintln!(
            "-- detected {engine}: this runtime does not provide {}, so {effect} is unavailable",
            profile.capability()
        ),
    }
    recipe
}

fn cmd_install(cli: &Cli, product_id: &str, dir: Option<&Path>) -> Result<ExitCode> {
    let cancellation = cancel::Cancellation::from_gui();
    if !is_product_id(product_id) {
        return Err(usage(&format!(
            "'{product_id}' is not a Store product id (12 characters, letters and digits, \
             e.g. 9NBLGGH2JHXJ -- it is in the Store URL)"
        )));
    }
    let product_id = normalise_product_id(product_id);

    let paths = Paths::from_env()?;
    let xodus_cli = paths
        .xodus_cli()
        .ok_or_else(|| anyhow!("no xodus-cli found; set XODUS_CLI_DIR (see: ferestre doctor)"))?;
    let (_, recipes) = load_recipes(cli).unwrap_or_else(|_| (PathBuf::new(), Vec::new()));
    let recipe = recipes.iter().find(|r| r.matches(&product_id));

    // A recipe knows the layout a title expects -- Bedrock's package lives one
    // level down -- so it decides the destination when there is one.
    let dest = match (dir, recipe) {
        (Some(d), _) => d.to_path_buf(),
        (None, Some(r)) => paths.install_dir(r)?,
        (None, None) => paths.games_dir().join(product_id.to_ascii_lowercase()),
    };

    let mut command = streaming_command(&xodus_cli, &product_id, &dest);
    if let Some(roles) = accounts::load(paths.config_dir())? {
        roles.apply(&mut command);
    }

    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "product_id": product_id,
            "destination": dest.display().to_string(),
            "plan": { "program": xodus_cli.display().to_string(),
                      "args": ["streaming", product_id, dest.display().to_string()] },
        }));
        return Ok(ExitCode::SUCCESS);
    }

    // Remembered so a failed install can put the disk back the way it found it.
    let dest_existed = dest.is_dir();
    std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;
    eprintln!(":: {product_id} -> {}", dest.display());
    let code = cancellation.run(command)?;
    // Keep partial files for the streaming client's resume path. In particular,
    // never remove an existing installation when an update is stopped.
    if cancellation.requested() {
        eprintln!(
            "-- download stopped; files kept at {} for resuming",
            dest.display()
        );
        return Ok(ExitCode::from(130));
    }

    // Checked whatever the exit code was, because both halves have been wrong.
    //
    // Exiting non-zero is the ordinary failure -- the client panicked partway
    // through a download -- and it leaves a directory behind that looks like a
    // half-finished install to anybody who goes looking.
    //
    // Exiting *zero* is the surprising one: the client prints "not entitled to
    // this content" and returns success when the account's licence does not
    // cover a title, which happens routinely with Game Pass, where the
    // catalogue lists far more than any one tier grants. Believing the exit
    // code there records a title as installed that is not, and it never
    // corrects itself: the row offers to launch nothing.
    if !install::looks_installed(&dest) {
        if code == 0 {
            eprintln!(
                "!! {product_id} did not install: nothing arrived in {}",
                dest.display()
            );
            eprintln!(
                "   the client exited successfully, so the reason is in its output above -- \
                 \"not entitled to this content\" means this account's licence does not \
                 cover the title, which for a Game Pass title means the subscription tier \
                 does not include it"
            );
        } else {
            eprintln!("!! {product_id} did not install: the client exited {code}");
        }
        // Clean up after ourselves, and only what we made: a directory that was
        // already there might be somebody's.
        if !dest_existed
            && install::safe_to_remove(&dest).is_ok()
            && std::fs::remove_dir_all(&dest).is_ok()
        {
            eprintln!("   removed the unfinished {}", dest.display());
        } else if dest_existed {
            eprintln!(
                "   {} was left as it is; `ferestre uninstall {product_id}` clears it",
                dest.display()
            );
        }
        return Ok(ExitCode::FAILURE);
    }
    if code != 0 {
        // Files arrived and the client still failed. Not recorded, because a
        // record is a claim that a title is ready to launch.
        eprintln!(
            "!! the client exited {code}; {} has files in it but the install did not finish",
            dest.display()
        );
        return Ok(ExitCode::from(code));
    }

    // A download replaces whatever was patched last time -- Microsoft's
    // XCurl.dll comes back with every update -- so the after-install steps run
    // again here, not once at first install.
    if let Some(recipe) = recipe {
        let dirs = launch_dirs(&paths)?;
        run_setup(recipe, "after-install", &dirs, &dest)?;
    }

    // Written last, and only on success, because a record for a download that
    // failed halfway would claim a build is installed that is not.
    cancellation.check()?;
    match record_install(&paths, &product_id, &dest) {
        Ok(record) => eprintln!("{}", installed_summary(&record)),
        // The title is installed either way. Failing the command over the
        // bookkeeping would be the tail wagging the dog.
        Err(e) => eprintln!("-- installed, but could not record the version: {e}"),
    }

    // A title nobody has described can describe itself now that it is on disk.
    // This is the moment it becomes possible: the package manifest names the
    // executable, and before the download there was no manifest to read. Doing
    // it here rather than in the window means `ferestre install <anything>`
    // leaves something runnable whichever way it was invoked.
    if recipe.is_none() {
        match write_detected_recipe(&paths, &product_id, &dest) {
            Ok(Some(path)) => eprintln!(
                "-- wrote a recipe from the package manifest: {}\n\
                    check it with `ferestre titles`, and edit it if the title \
                 needs something else",
                path.display()
            ),
            Ok(None) => eprintln!(
                "-- no recipe, and the package manifest does not name an executable; \
                 write one by hand: see titles/SCHEMA.md"
            ),
            Err(e) => eprintln!("-- could not write a recipe: {e}"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Remove a title: the files, and the record that says it is installed.
///
/// Both, because either on its own leaves a lie behind. Files without a record
/// is a directory nothing knows about; a record without files is a row offering
/// to launch nothing, which is exactly the state that made this command
/// necessary -- an install failed while the client exited zero, and there was
/// no way to say so.
fn cmd_uninstall(
    cli: &Cli,
    product_id: &str,
    keep_files: bool,
    remove_prefix: bool,
    yes: bool,
) -> Result<ExitCode> {
    let product_id = normalise_product_id(product_id);
    let paths = Paths::from_env()?;

    // The record first, then the recipe: the record is what the launcher acted
    // on, and a recipe's install directory is where it *would* have gone.
    let record = install::load(paths.state_dir(), &product_id);
    let (_, recipes) = load_recipes(cli).unwrap_or_else(|_| (PathBuf::new(), Vec::new()));
    let dir = match &record {
        Some(record) => Some(record.dir.clone()),
        None => recipes
            .iter()
            .find(|r| r.matches(&product_id))
            .and_then(|r| paths.install_dir(r).ok())
            // Neither recorded nor described, but a directory may still be
            // there: this is exactly what a failed install leaves, and refusing
            // to touch it means the only thing that can clean it up is `rm`.
            // The same rule `install` uses when it has no recipe to follow.
            .or_else(|| {
                let guess = paths.games_dir().join(product_id.to_ascii_lowercase());
                guess.is_dir().then_some(guess)
            }),
    };

    let Some(dir) = dir else {
        bail!("nothing is recorded or described for {product_id}, so there is nothing to remove");
    };
    let removing = !keep_files && dir.is_dir();

    // The Wine prefix is a separate directory and it outlives the install: it
    // is where a title's saved games are, so removing it silently alongside the
    // files would throw away the thing somebody most wants to keep. Named
    // either way, because a directory left behind that nothing mentions is how
    // `stardew-valley-proton` sat there after Stardew Valley was uninstalled.
    let prefix = recipes
        .iter()
        .find(|r| r.matches(&product_id))
        .and_then(|r| paths.prefix_dir(r).ok())
        .filter(|p| p.is_dir());

    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "product_id": product_id,
            "directory": dir.display().to_string(),
            "removing_files": removing,
            "had_record": record.is_some(),
            "prefix": prefix.as_ref().map(|p| p.display().to_string()),
            "removing_prefix": remove_prefix && prefix.is_some(),
        }));
        return Ok(ExitCode::SUCCESS);
    }

    if removing {
        install::safe_to_remove(&dir)?;
        let size = install::tree_size(&dir);
        eprintln!(
            ":: removing {} ({})",
            dir.display(),
            ferestre_core::human_bytes(size)
        );
        if !yes {
            // Asked, not assumed. This is the only command here that destroys
            // something, and `--yes` exists for the window, which asks first in
            // its own dialog.
            eprint!("   delete it? [y/N] ");
            use std::io::Write;
            std::io::stderr().flush().ok();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim(), "y" | "Y" | "yes") {
                eprintln!("-- left alone");
                return Ok(ExitCode::FAILURE);
            }
        }
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        eprintln!("-- removed {}", dir.display());
    } else if !keep_files {
        eprintln!(":: {} is already gone", dir.display());
    }

    if let Some(prefix) = &prefix {
        if remove_prefix {
            install::safe_to_remove(prefix)?;
            std::fs::remove_dir_all(prefix)
                .with_context(|| format!("removing {}", prefix.display()))?;
            eprintln!("-- removed the Wine prefix {}", prefix.display());
        } else {
            eprintln!(
                "-- kept the Wine prefix {} ({}), which is where saved games live",
                prefix.display(),
                ferestre_core::human_bytes(install::tree_size(prefix))
            );
            eprintln!("   remove it too with: ferestre uninstall {product_id} --prefix");
        }
    }

    install::forget(paths.state_dir(), &product_id)?;
    eprintln!("-- forgot the install record for {product_id}");
    if keep_files {
        eprintln!("   the files are still in {}", dir.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// Tell the prefix about the WinRT classes this runtime hosts.
///
/// Best effort on purpose: a class that cannot be registered is a title that
/// may still run, and failing a launch over it would trade a definite loss for
/// a possible one. What it must not do is stay quiet -- if a title then dies in
/// `RoGetActivationFactory`, the reason is on screen above it.
fn register_winrt_classes(prefix: &Path, registry: &Registry, runtime: &InstalledRuntime) {
    let classes = registry.winrt_classes();
    if classes.is_empty() {
        return;
    }
    // A prefix that does not exist yet is about to be created from the default
    // one, and that copy would overwrite anything written now.
    if !winrt::registry_path(prefix).is_file() {
        return;
    }
    let missing = winrt::missing_in_prefix(prefix, &classes);
    if missing.is_empty() {
        return;
    }

    // Asked before `wine reg` runs, because afterwards there is always one.
    let wine_prefix = prefix.join("pfx");
    let session_before = wineserver_running(&wine_prefix);
    for class in missing {
        let status = Command::new(runtime.path.join("files/bin/wine"))
            .args([
                "reg",
                "add",
                &class.key(),
                "/v",
                "DllPath",
                "/d",
                &class.dll_path(),
                "/f",
            ])
            .env("WINEPREFIX", &wine_prefix)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {
                eprintln!("-- registered {} with the prefix", class.name)
            }
            Ok(status) => eprintln!("-- could not register {}: reg exited {status}", class.name),
            Err(e) => eprintln!("-- could not register {}: {e}", class.name),
        }
    }
    // `wine reg` started a session for the prefix -- services, winedevice and
    // the input bus -- with this process's environment, not the title's. A
    // title launched while it lingers joins it, and its input settings (the
    // shim's PROTON_DISABLE_HIDRAW, a recipe's override) never reach the bus
    // that enumerates controllers. Let it end first -- but only a session
    // started here, and not forever: `wineserver -w` waits for every process
    // in the session, and a session that was already there may be this very
    // title, still running, or one whose Quit never returned.
    const STILL_RUNNING: &str =
        "-- input settings may not apply: a Wine session for this prefix is still running";
    if session_before == Some(true) {
        eprintln!("{STILL_RUNNING}");
        return;
    }
    let mut wait = Command::new(runtime.path.join("files/bin/wineserver"));
    wait.arg("-w")
        .env("WINEPREFIX", &wine_prefix)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(mut wait) = wait.spawn() {
        if !wait_at_most(&mut wait, std::time::Duration::from_secs(15)) {
            eprintln!("{STILL_RUNNING}");
        }
    }
}

/// Whether a wineserver runs for this prefix. Wine keys its server directory
/// by the prefix's device and inode, and the server holds a write lock on the
/// `lock` file in it for as long as it runs. Testing that lock is passive;
/// connecting to the server's socket would register a client with it. `None`
/// when it cannot be told.
fn wineserver_running(prefix: &Path) -> Option<bool> {
    // Safety: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    wineserver_running_in(Path::new(&format!("/tmp/.wine-{uid}")), prefix)
}

fn wineserver_running_in(base: &Path, prefix: &Path) -> Option<bool> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(prefix).ok()?;
    let lock = base
        .join(format!("server-{:x}-{:x}", metadata.dev(), metadata.ino()))
        .join("lock");
    let file = match std::fs::File::open(&lock) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    // Safety: a zeroed flock is a valid argument, and F_GETLK only fills it in.
    let mut probe: libc::flock = unsafe { std::mem::zeroed() };
    probe.l_type = libc::F_WRLCK as _;
    probe.l_whence = libc::SEEK_SET as _;
    probe.l_start = 0;
    probe.l_len = 1;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut probe) } != 0 {
        return None;
    }
    Some(libc::c_int::from(probe.l_type) != libc::F_UNLCK)
}

/// Wait for a child, giving up on it after `limit`. True if it finished in
/// time; one that did not is killed and reaped, so nothing is left waiting
/// behind the launch.
fn wait_at_most(child: &mut std::process::Child, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100))
            }
            _ => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

/// Describe an installed title from its own package, if it is installed and the
/// package names an executable.
///
/// `Ok(None)` covers three honest cases that are all "there is nothing to write
/// down": nothing recorded this product, the directory it recorded is gone, or
/// the manifest names no entry point.
fn describe_from_disk(paths: &Paths, title: &str) -> Result<Option<PathBuf>> {
    let product_id = normalise_product_id(title);
    let Some(record) = install::load(paths.state_dir(), &product_id) else {
        return Ok(None);
    };
    if !install::looks_installed(&record.dir) {
        return Ok(None);
    }
    write_detected_recipe(paths, &product_id, &record.dir)
}

/// Write a recipe for a title that had none, from what the package says.
///
/// `Ok(None)` when the manifest names no executable -- honest rather than a
/// guess, because a recipe pointing at the wrong binary produces a launch
/// failure that looks like the launcher's fault rather than an unfinished
/// description.
fn write_detected_recipe(paths: &Paths, product_id: &str, dest: &Path) -> Result<Option<PathBuf>> {
    let Some(executable) = install::executable(dest) else {
        return Ok(None);
    };

    // The catalog knows the name; without it the product id is still a name.
    let (market, language) = paths.market();
    let (products, _) = catalog::resolve(
        &paths.catalog_cache(),
        &[product_id.to_string()],
        &market,
        &language,
    );
    let name = products
        .iter()
        .find(|p| p.product_id.eq_ignore_ascii_case(product_id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| product_id.to_string());

    let mut recipe = Recipe::blank(product_id, &name);
    recipe.launch.executable = executable;
    // The download went to games_dir/<product id>, not to the slug the blank
    // recipe would default to, so the recipe has to say where it actually is.
    if let Ok(relative) = dest.strip_prefix(paths.games_dir()) {
        recipe.install.dir = Some(relative.to_string_lossy().into_owned());
    }
    detected_requirements(&mut recipe, dest);
    let path = recipe.save_to(&paths.user_titles_dir())?;
    Ok(Some(path))
}

/// What a recipe written from a package can honestly ask of the runtime.
///
/// `requires` only names what every runtime this launcher would pick can be
/// seen to provide, because `runtime::select` refuses a runtime that misses a
/// requirement before the gate ever gets to say "could not be verified". A
/// runtime that publishes nothing is probed, and the probe can see the
/// inherited-fd change and our `xgameruntime.dll` but never package identity.
/// So:
///
/// - every package is an encrypted MSIXVC image, which cannot start without
///   the memfd main image: required, and the probe can confirm it;
/// - a GDK package (it has `MicrosoftGame.config`) signs a user in through
///   our `xgameruntime.dll`: required, and the probe can confirm that too;
/// - the same package asks Wine who it is, which a probe cannot see: wanted,
///   so a published list that lacks it still warns, and a probed runtime is
///   not refused for something nobody could check.
///
/// The engine's own wants go in as well, so the file shows what the launcher
/// will ask for.
fn detected_requirements(recipe: &mut Recipe, dest: &Path) {
    recipe.runtime.requires = vec![capability::MEMFD_MAIN_IMAGE.into()];
    if install::game_config(dest).is_some() {
        recipe
            .runtime
            .requires
            .push(capability::XGAMERUNTIME_USER.into());
        recipe
            .runtime
            .wants
            .push(capability::PACKAGE_IDENTITY.into());
    }
    if let Some(profile) = autofix::detect(dest, &recipe.launch.executable) {
        autofix::add_wants(recipe, profile);
    }
}

/// Write down what was just installed, so an update can be detected later.
///
/// Read from the install itself, not from the catalog: the client leaves the
/// package header on disk and the GUID in it is the content id. That means the
/// record describes what is actually there, needs no network, and is the same
/// operation that adopts a title installed before this launcher existed.
fn record_install(paths: &Paths, product_id: &str, dest: &Path) -> Result<install::Record> {
    let mut record = install::adopt(product_id, dest).unwrap_or_else(|| install::Record {
        product_id: product_id.to_string(),
        dir: dest.to_path_buf(),
        // No header to read, so no content id to claim. `update_available`
        // reports that as "cannot tell", which is the truth.
        content_ids: Vec::new(),
        installed_at: None,
        package_version: install::package_version(dest),
    });
    // This install we did watch happen, so the date is known rather than guessed.
    record.installed_at = install::now_rfc3339();
    install::save(paths.state_dir(), &record)?;
    Ok(record)
}

fn installed_summary(record: &install::Record) -> String {
    let version = match &record.package_version {
        Some(v) => format!(" version {v}"),
        None => String::new(),
    };
    if record.content_ids.is_empty() {
        format!(
            "-- recorded the install{version}, but the package header was not readable, \
             so updates cannot be detected for it"
        )
    } else {
        format!("-- recorded the install{version}; updates will be detected")
    }
}

/// GitHub's release feed is public. Runtime tags share the same repository as
/// the small launcher releases, and `runtime::latest_release` filters the two
/// deliberately rather than relying on a mutable "latest" link.
const RELEASES_API: &str = "https://api.github.com/repos/icex/ferestre/releases?per_page=30";

fn cmd_install_runtime(cli: &Cli) -> Result<ExitCode> {
    let cancellation = cancel::Cancellation::from_gui();
    let paths = Paths::from_env()?;
    eprintln!("-- checking for the latest Ferestre runtime");
    let release = runtime::latest_release(&http::get_text(RELEASES_API)?)?;
    cancellation.check()?;
    let version = release
        .tag
        .strip_prefix("runtime-")
        .filter(|version| {
            !version.is_empty()
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        })
        .ok_or_else(|| anyhow!("runtime release has an unsafe version: {}", release.tag))?;
    let destination = paths.runtimes_dir().join(version);
    if InstalledRuntime::at(&destination).is_ok() {
        println!(
            "-- runtime {version} is already installed at {}",
            destination.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "release": release.tag,
            "asset": release.asset_name,
            "destination": destination,
        }));
        return Ok(ExitCode::SUCCESS);
    }

    let archive = paths.cache_dir().join("runtime").join(&release.asset_name);
    eprintln!("-- downloading runtime {version} (this is a one-time download)");
    http::download_large_cancellable(&release.url, &archive, || cancellation.requested())?;
    cancellation.check()?;
    eprintln!("-- unpacking runtime {version}");
    std::fs::create_dir_all(paths.runtimes_dir())?;
    let temporary = paths
        .runtimes_dir()
        .join(format!(".{version}.installing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temporary);
    std::fs::create_dir_all(&temporary)?;
    let mut unpack = Command::new("tar");
    unpack
        .args(["-xJf"])
        .arg(&archive)
        .arg("-C")
        .arg(&temporary);
    let code = cancellation.run(unpack);
    if cancellation.requested() || !matches!(code, Ok(0)) {
        let _ = std::fs::remove_dir_all(&temporary);
        cancellation.check()?;
        bail!(
            "could not unpack {}; tar exited {}",
            archive.display(),
            code?
        );
    }
    let unpacked = temporary.join(format!("ferestre-runtime-{version}"));
    InstalledRuntime::at(&unpacked)?;
    if destination.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
        bail!(
            "{} appeared while the runtime was downloading; try again",
            destination.display()
        );
    }
    std::fs::rename(&unpacked, &destination)?;
    let _ = std::fs::remove_dir_all(&temporary);
    println!(
        "-- installed runtime {version} at {}",
        destination.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_env(cli: &Cli) -> Result<ExitCode> {
    let paths = Paths::from_env()?;
    let vars = resolved_env(&paths);

    if cli.json {
        let map: serde_json::Map<String, Value> = vars
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    Value::String(env_display(k, v, cli.show_account)),
                )
            })
            .collect();
        print_json(&json!({ "schema": JSON_SCHEMA, "env": map }));
        return Ok(ExitCode::SUCCESS);
    }
    for (name, value) in &vars {
        println!("{name}={}", env_display(name, value, cli.show_account));
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// wiring to ferestre-core
// ---------------------------------------------------------------------------

/// The recipes, and where they came from. `--titles-dir` wins: it is how
/// someone tests a recipe they are writing without installing it first.
/// The recipes, packaged ones overlaid with the reader's own.
///
/// Layered, and that is not a detail: the window's editor writes to the user's
/// titles directory, and the window launches by running this binary -- so while
/// this read only the packaged directory, every edit somebody saved was
/// silently ignored at the moment it was supposed to take effect. The same bug
/// made a recipe written from a package manifest during `run` invisible to the
/// `run` that had just written it.
///
/// `--titles-dir` still means exactly that one directory, because it exists for
/// testing a directory in isolation.
fn load_recipes(cli: &Cli) -> Result<(PathBuf, Vec<Recipe>)> {
    if let Some(dir) = &cli.titles_dir {
        let recipes = Recipe::load_dir(dir)
            .with_context(|| format!("reading recipes from {}", dir.display()))?;
        return Ok((dir.clone(), recipes));
    }

    let paths = Paths::from_env()?;
    let dir = paths
        .titles_dir()
        .ok_or_else(|| {
            anyhow!("cannot find the title recipes; pass --titles-dir <DIR> or set XODUS_REPO_DIR")
        })?
        .to_path_buf();
    let dirs = paths.title_dirs();
    let recipes =
        Recipe::load_layered(&dirs).with_context(|| format!("reading recipes from {dirs:?}"))?;
    Ok((dir, recipes))
}

/// `titles/capabilities.toml`, which turns "missing gameinput.v2" into the
/// message the title would actually have printed. Optional everywhere, so a
/// failure to read it is not worth failing a launch over.
fn registry_for(paths: &Paths) -> Option<Registry> {
    let file = paths.titles_dir()?.join("capabilities.toml");
    Registry::load(&file).ok()
}

fn launch_dirs(paths: &Paths) -> Result<launch::Dirs> {
    let runtime = paths
        .runtime_dir()
        .ok_or_else(|| anyhow!("no patched Proton found; run: ferestre install-runtime"))?;
    launch_dirs_for_runtime(paths, runtime)
}

fn launch_dirs_for_runtime(paths: &Paths, runtime: &Path) -> Result<launch::Dirs> {
    Ok(launch::Dirs {
        games_dir: paths.games_dir().to_path_buf(),
        cli_dir: paths
            .cli_dir()
            .ok_or_else(|| anyhow!("no xodus-cli found; set XODUS_CLI_DIR (see: ferestre doctor)"))?
            .to_path_buf(),
        proton_dir: runtime.to_path_buf(),
        // launch-gdk.sh falls back to the conventional path rather than leaving
        // STEAM_COMPAT_CLIENT_INSTALL_PATH empty, and Proton wants *something*.
        steam_dir: paths
            .steam_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| paths.home().join(".steam/steam")),
        scripts_dir: paths
            .scripts_dir()
            .ok_or_else(|| anyhow!("cannot find the scripts directory; set XODUS_REPO_DIR"))?
            .to_path_buf(),
    })
}

/// What `ferestre env` prints, in the order `scripts/xodus-env.sh` documents them.
fn resolved_env(paths: &Paths) -> Vec<(&'static str, String)> {
    let show = |p: Option<&Path>| p.map(|p| p.display().to_string()).unwrap_or_default();
    let mut vars = vec![
        ("XODUS_REPO_DIR", show(paths.tree_dir())),
        ("XODUS_GAMES_DIR", paths.games_dir().display().to_string()),
        ("XODUS_BUILD_DIR", paths.build_dir().display().to_string()),
        ("XODUS_CLI_DIR", show(paths.cli_dir())),
        ("XODUS_STEAM_DIR", show(paths.steam_dir())),
        ("XODUS_PROTON_DIR", show(paths.runtime_dir())),
        ("FERESTRE_TITLES_DIR", show(paths.titles_dir())),
        (
            "FERESTRE_STATE_DIR",
            paths.state_dir().display().to_string(),
        ),
        (
            "FERESTRE_CACHE_DIR",
            paths.cache_dir().display().to_string(),
        ),
    ];
    // Only if it is set: an empty XGR_XUID line invites someone to fill it in
    // with the wrong thing.
    if let Some(xuid) = paths.var("XGR_XUID") {
        vars.push(("XGR_XUID", xuid.to_string()));
    }
    vars
}

// ---------------------------------------------------------------------------
// child processes -- the part this binary genuinely owns
// ---------------------------------------------------------------------------

/// Run a child in the foreground, sharing our terminal, and become its status.
///
/// Foreground, always: inside an AppImage the mount lives exactly as long as
/// this process, and `proton-wine-shim.sh` -- which the client execs as its
/// "wine" -- is on that mount. Daemonising would pull the scripts out from
/// under the running game.
fn run_inherited(mut command: Command) -> Result<u8> {
    let status = command
        .status()
        .with_context(|| format!("running {:?}", command.get_program()))?;
    Ok(exit_code_from(status.code(), signal_of(&status)))
}

/// Run a child, copying its output to our terminal and to a log file at once.
///
/// `launch-gdk.sh` does this with `exec > >(tee -a "$LOG")`, and the log is not
/// decoration: `[[issues]].log-match` in a recipe exists to recognise a failure
/// in it rather than re-diagnose one.
fn run_teed(mut command: Command, log: &Path, forwarding: &Forwarding) -> Result<ExitCode> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("opening {}", log.display()))?;
    let file = Arc::new(Mutex::new(file));
    {
        let mut f = file.lock().expect("log mutex");
        let _ = writeln!(f, "=== launch (unix {}) ===", unix_seconds());
    }

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("running {:?}", command.get_program()))?;
    let out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");
    let t_out = tee(out, Arc::clone(&file), false);
    let t_err = tee(err, Arc::clone(&file), true);

    let status = forwarding
        .wait(&mut child)
        .context("waiting for the title")?;
    let _ = t_out.join();
    let _ = t_err.join();

    let code = exit_code_from(status.code(), signal_of(&status));
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "=== exited rc={code} ===");
    }
    if code != 0 {
        eprintln!(
            "-- the title exited {code}; the log is at {}",
            log.display()
        );
    }
    Ok(ExitCode::from(code))
}

fn tee<R: std::io::Read + Send + 'static>(
    reader: R,
    log: Arc<Mutex<std::fs::File>>,
    is_stderr: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            if is_stderr {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
            if let Ok(mut f) = log.lock() {
                let _ = writeln!(f, "{line}");
            }
        }
    })
}

/// The signals that end a launch from outside: the window's Stop (SIGTERM),
/// Ctrl+C (SIGINT), a terminal closing (SIGHUP), and a plain `kill`.
const LAUNCH_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

static SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static SIGNAL_RECEIVED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static FORWARDING: std::sync::OnceLock<Arc<Mutex<Option<i32>>>> = std::sync::OnceLock::new();

extern "C" fn on_launch_signal(signal: libc::c_int) {
    use std::sync::atomic::Ordering;
    SIGNAL_RECEIVED.store(signal, Ordering::SeqCst);
    let fd = SIGNAL_PIPE.load(Ordering::SeqCst);
    if fd >= 0 {
        // Safety: write is async-signal-safe, and errno is put back for
        // whatever the signal interrupted.
        unsafe {
            let errno = *libc::__errno_location();
            let byte = signal as u8;
            libc::write(fd, (&byte as *const u8).cast(), 1);
            *libc::__errno_location() = errno;
        }
    }
}

/// Signals that would end a launch, passed on to its title instead.
///
/// Ended by a signal, this process never unwinds: its claim on the service is
/// never dropped, and the service is never stopped. Caught, the signal goes to
/// the title, which ends; the launch then finishes the ordinary way, and the
/// last title out still stops the service. Handlers do not survive exec, so
/// every child starts with the default dispositions.
struct Forwarding {
    title: Arc<Mutex<Option<i32>>>,
}

impl Forwarding {
    fn install() -> Result<Self> {
        if let Some(title) = FORWARDING.get() {
            return Ok(Self {
                title: Arc::clone(title),
            });
        }
        use std::os::fd::FromRawFd;
        let mut fds = [0; 2];
        // Safety: pipe2 fills in two new descriptors.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(std::io::Error::last_os_error()).context("creating the signal pipe");
        }
        // Safety: the read end is ours alone. The write end lives as long as
        // the process, for the handler; it must never block in one.
        let mut read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        unsafe { libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) };
        SIGNAL_PIPE.store(fds[1], std::sync::atomic::Ordering::SeqCst);

        let title = Arc::new(Mutex::new(None::<i32>));
        let forwarded = Arc::clone(&title);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut byte = [0u8];
            loop {
                match read.read(&mut byte) {
                    Ok(1) => {
                        // Keep the guard across the kill: `wait` clears the
                        // pid under the same lock before it reaps the title,
                        // so the pid cannot be reaped and reused in between.
                        let guard = forwarded.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(pid) = *guard {
                            // Safety: an unreaped child of ours while the
                            // guard is held; see `wait`.
                            unsafe { libc::kill(pid, libc::c_int::from(byte[0])) };
                        }
                        drop(guard);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => break,
                }
            }
        });
        for signal in LAUNCH_SIGNALS {
            // Safety: a handler that only stores and writes, installed with a
            // fully initialised sigaction.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_launch_signal as extern "C" fn(libc::c_int) as usize;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error()).context("handling signals");
                }
            }
        }
        let _ = FORWARDING.set(Arc::clone(&title));
        Ok(Self { title })
    }

    /// The last signal caught, if any.
    fn received(&self) -> Option<i32> {
        Some(SIGNAL_RECEIVED.load(std::sync::atomic::Ordering::SeqCst)).filter(|s| *s != 0)
    }

    /// Wait for the title, passing signals on to it meanwhile.
    fn wait(&self, child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
        let pid = child.id() as i32;
        *self.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid);
        // One that came before there was a title to pass it to.
        if let Some(signal) = self.received() {
            // Safety: our own child, not yet reaped.
            unsafe { libc::kill(pid, signal) };
        }
        // Wait without reaping, so the pid cannot be reused while it may
        // still be signalled.
        loop {
            // Safety: waitid only fills in `info`.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let waited = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if waited == 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break;
            }
        }
        *self.title.lock().unwrap_or_else(|e| e.into_inner()) = None;
        child.wait()
    }
}

const SERVICE_SOCKET_OVERRIDE: &str = "xgameruntime.service-socket-override";

fn scoped_service_directory(supported: bool, id: &str, root: &Path) -> Result<Option<PathBuf>> {
    if !supported {
        return Ok(None);
    }
    let id = id.trim();
    if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid packaged Xbox service fingerprint");
    }
    // Keep the Unix socket path below sun_path limits (the full SHA remains
    // in SERVICE_ID; 128 bits identify the namespace).
    Ok(Some(root.join("ferestre-services").join(&id[..32])))
}

/// A capable runtime and its packaged service share a content-addressed socket
/// directory. Replacing the AppImage can start the updated service without
/// interrupting a game connected to the previous one. Older runtimes retain
/// their existing socket because they do not understand the directory override.
fn plan_service_socket(
    plan: &mut launch::Plan,
    dirs: &launch::Dirs,
    runtime: &InstalledRuntime,
    roles: Option<&accounts::Selection>,
) -> Result<Option<PathBuf>> {
    if let Some(roles) = roles {
        let id = match std::fs::read_to_string(dirs.cli_dir.join("SERVICE_ID")) {
            Ok(id) => id,
            Err(_) => accounts::service_fingerprint(&dirs.xodus_service())?,
        };
        let binding = format!(
            "{}:{}:{}",
            id.trim(),
            plan.get("XODUS_STORE_PRODUCT").unwrap_or_default(),
            plan.get("XODUS_STORE_CONTENT").unwrap_or_default()
        );
        let root = std::env::var_os("XODUS_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
            .context("XDG_RUNTIME_DIR is required for account sharing")?;
        let directory =
            scoped_service_directory(true, &roles.service_key(&binding), Path::new(&root))?
                .unwrap();
        plan.env_set.insert(
            "XODUS_RUNTIME_DIR".into(),
            directory.to_string_lossy().into_owned(),
        );
        return Ok(Some(directory.join("xodus.sock")));
    }
    if let Some(value) = plan.get("XODUS_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return Ok(Some(PathBuf::from(value).join("xodus.sock")));
    }
    if let Some(value) = std::env::var_os("XODUS_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return Ok(Some(PathBuf::from(value).join("xodus.sock")));
    }
    if runtime.provides(SERVICE_SOCKET_OVERRIDE) {
        let metadata = dirs.cli_dir.join("SERVICE_ID");
        match std::fs::read_to_string(&metadata) {
            Ok(id) => {
                if let Some(root) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
                    let directory = scoped_service_directory(true, &id, Path::new(&root))?.unwrap();
                    plan.env_set.insert(
                        "XODUS_RUNTIME_DIR".into(),
                        directory.to_string_lossy().into_owned(),
                    );
                    return Ok(Some(directory.join("xodus.sock")));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", metadata.display()))
            }
        }
    }
    Ok(runtime_socket())
}

const SERVICE_PID: &str = "xodus-service.pid";

/// Open (creating it) a lock file only this user can read.
fn open_lock(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

/// Take a flock, blocking. False, with a warning saying what is lost, where
/// the filesystem has no flock at all -- NFS without lockd, some FUSE mounts.
/// `XODUS_RUNTIME_DIR` can point anywhere, and a launch that worked before
/// the locks existed should not stop working because of them.
fn flock_or_warn(
    file: &std::fs::File,
    operation: libc::c_int,
    path: &Path,
    without: &str,
) -> Result<bool> {
    use std::os::fd::AsRawFd;
    loop {
        // Safety: a plain flock on a descriptor the caller owns.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::ENOLCK | libc::EOPNOTSUPP | libc::ENOSYS) => {
                eprintln!(
                    "-- {} cannot be locked ({error}); {without}",
                    path.display()
                );
                return Ok(false);
            }
            _ => return Err(error).with_context(|| format!("locking {}", path.display())),
        }
    }
}

/// A launch's claim on the service its title talks to.
///
/// The service used to outlive every title. Steam runs a shortcut under a
/// subreaper and shows it as running until every descendant has exited, so a
/// service left behind kept the shortcut "running" for hours and Steam would
/// not start it again. Each launch holds a shared lock while its title runs;
/// the last one to finish stops the service. A launch takes its lock before
/// looking for a live service, so it never reuses one that is being stopped.
/// `launch-gdk.sh` takes the same shared lock, so a title it started keeps a
/// service this launcher started.
struct ServiceUse {
    lock: std::fs::File,
    directory: PathBuf,
}

impl ServiceUse {
    fn begin(socket: Option<&Path>) -> Result<Option<Self>> {
        let Some(directory) = socket.and_then(Path::parent) else {
            return Ok(None);
        };
        std::fs::create_dir_all(directory)
            .with_context(|| format!("creating {}", directory.display()))?;
        let path = directory.join("users.lock");
        let lock = open_lock(&path)?;
        if !flock_or_warn(
            &lock,
            libc::LOCK_SH,
            &path,
            "the Xbox service will be left running after the title",
        )? {
            return Ok(None);
        }
        Ok(Some(Self {
            lock,
            directory: directory.to_path_buf(),
        }))
    }
}

impl Drop for ServiceUse {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // Safety: flock on descriptors owned here.
        unsafe { libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN) };
        let Ok(probe) = std::fs::File::open(self.directory.join("users.lock")) else {
            return;
        };
        if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return; // another title still uses it
        }
        // The exclusive lock is held until `probe` closes, after the service
        // has gone: a launch waiting to begin then starts a new one.
        stop_service(&self.directory);
    }
}

/// Stop the service recorded in `directory`, if it is still that service. A
/// service started by an older launcher left no pid and is left alone.
///
/// The pid file can outlive its service -- a service killed some other way
/// never removes it -- and the pid can then belong to anything, another
/// directory's service included, since every version is named xodus-service.
/// A service runs in the directory it was started for, so that is checked
/// too, and the signal goes through a pidfd where the kernel has them: it
/// reaches the process that was checked, or nothing.
fn stop_service(directory: &Path) {
    let pid_file = directory.join(SERVICE_PID);
    let Some(pid) = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|pid| pid.trim().parse::<i32>().ok())
        .filter(|pid| *pid > 1)
    else {
        return;
    };
    let home = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    let ours = || service_in(pid, &home);
    if let Some(process) = Process::open(pid) {
        if ours() {
            process.signal(libc::SIGTERM);
            if !gone_within(&ours, std::time::Duration::from_secs(3)) {
                process.signal(libc::SIGKILL);
                // Not let go of until it has: a launch waiting for the lock
                // would otherwise find the dying service still listening.
                gone_within(&ours, std::time::Duration::from_secs(2));
            }
        }
    }
    let _ = std::fs::remove_file(pid_file);
}

/// Whether `pid` is a running xodus-service working in `directory`. An exited
/// one waits as a zombie until its parent reaps it, under the same name, and
/// does not count.
fn service_in(pid: i32, directory: &Path) -> bool {
    let named = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.split_once(" (")
            .and_then(|(_, rest)| rest.rsplit_once(") "))
            .is_some_and(|(comm, rest)| comm == "xodus-service" && !rest.starts_with('Z'))
    });
    named && std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd == directory)
}

fn gone_within(running: &dyn Fn() -> bool, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while running() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    true
}

/// A process to signal, held by a pidfd when the kernel offers one (5.3 and
/// later), and by its pid otherwise.
struct Process {
    pid: i32,
    pidfd: Option<std::os::fd::OwnedFd>,
}

impl Process {
    /// `None` when there is no such process any more.
    fn open(pid: i32) -> Option<Self> {
        use std::os::fd::FromRawFd;
        // Safety: pidfd_open takes a pid and flags and returns a new descriptor.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd >= 0 {
            // Safety: a descriptor just returned to us and owned by nothing else.
            let pidfd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) };
            return Some(Self {
                pid,
                pidfd: Some(pidfd),
            });
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => None,
            _ => Some(Self { pid, pidfd: None }),
        }
    }

    fn signal(&self, signal: libc::c_int) {
        use std::os::fd::AsRawFd;
        // Safety: signals sent to a process just checked to be the service.
        match &self.pidfd {
            Some(pidfd) => unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            },
            None => unsafe {
                libc::kill(self.pid, signal);
            },
        }
    }
}

fn is_service_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit())
}

/// The service executable to start, outside an AppImage mount.
///
/// A service runs on after the title that started it. Started from inside the
/// AppImage, it maps its executable from the mount and keeps it busy, so the
/// AppImage process never exits: Steam then shows the shortcut as still
/// running and will not start it again. A copy, keyed by the packaged
/// `SERVICE_ID`, runs from the cache instead. Without an id there is no safe
/// key, and the packaged binary is used as before.
fn detached_service(
    binary: &Path,
    appdir: Option<&Path>,
    cli_dir: &Path,
    cache: &Path,
) -> Result<PathBuf> {
    if !appdir.is_some_and(|appdir| binary.starts_with(appdir)) {
        return Ok(binary.to_path_buf());
    }
    let Some(id) = std::fs::read_to_string(cli_dir.join("SERVICE_ID"))
        .ok()
        .map(|id| id.trim().to_ascii_lowercase())
        .filter(|id| is_service_id(id))
    else {
        return Ok(binary.to_path_buf());
    };
    let services = cache.join("services");
    let directory = services.join(&id);
    let copy = directory.join("xodus-service");
    let size = std::fs::metadata(binary)
        .with_context(|| format!("reading {}", binary.display()))?
        .len();
    if std::fs::metadata(&copy).is_ok_and(|m| m.is_file() && m.len() == size) {
        // Marks the copy as recently used, so another version's cleanup
        // leaves it alone while this launch starts it.
        let _ = std::fs::File::open(&directory)
            .and_then(|d| d.set_modified(std::time::SystemTime::now()));
        return Ok(copy);
    }
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    remove_abandoned_copies(&directory);
    let partial = directory.join(format!(".xodus-service.{}", std::process::id()));
    let published = (|| {
        std::fs::copy(binary, &partial).with_context(|| format!("copying {}", binary.display()))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", partial.display()))?;
        std::fs::rename(&partial, &copy).with_context(|| format!("publishing {}", copy.display()))
    })();
    if let Err(error) = published {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    remove_other_versions(&services, &id, &running_executables());
    Ok(copy)
}

/// Copies left half-written by a launch that died while making them.
fn remove_abandoned_copies(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".xodus-service."))
        else {
            continue;
        };
        if pid.parse::<u32>().is_ok() && !Path::new("/proc").join(pid).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Earlier versions' copies, once nothing runs them. Every AppImage update
/// that changes the service adds a full-size copy, and nothing else removes
/// them. One used in the last few minutes is kept as well: another version's
/// launch may be about to start it.
fn remove_other_versions(services: &Path, keep: &str, running: &[PathBuf]) {
    const RECENT: std::time::Duration = std::time::Duration::from_secs(600);
    let Ok(entries) = std::fs::read_dir(services) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .map_or(true, |name| name == keep || !is_service_id(name))
        {
            continue;
        }
        let recent = entry
            .metadata()
            .and_then(|m| m.modified())
            .map_or(true, |modified| {
                !modified.elapsed().is_ok_and(|age| age >= RECENT)
            });
        let Ok(path) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if recent || running.iter().any(|exe| exe.starts_with(&path)) {
            continue;
        }
        let _ = std::fs::remove_dir_all(path);
    }
}

/// The executables of the processes this user can see.
fn running_executables() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .filter_map(|entry| std::fs::read_link(entry.path().join("exe")).ok())
        .collect()
}

/// How to start the service, detached from this launch.
///
/// The service outlives this launch. Anything it inherits stays open as long
/// as it runs -- the AppImage runtime leaves a descriptor on its own mount --
/// so only the three standard streams go with it. The rest are marked
/// close-on-exec rather than closed: one of them is the pipe through which
/// `spawn` learns that exec failed, and closing it turns a missing or
/// unrunnable executable into a child that aborts and a spawn that "worked".
///
/// It gets its own process group, too. A window stopping one title signals
/// that title's group, and so does Ctrl+C in a terminal; a service another
/// title still uses must not go with it.
fn service_command(
    executable: &Path,
    directory: &Path,
    log: &std::fs::File,
) -> std::io::Result<Command> {
    use std::os::unix::process::CommandExt;
    let mut service = Command::new(executable);
    service
        .env("XODUS_RUNTIME_DIR", directory)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?))
        .process_group(0);
    // Safety: async-signal-safe system calls only, between fork and exec.
    unsafe {
        service.pre_exec(|| {
            let cloexec = libc::CLOSE_RANGE_CLOEXEC as libc::c_uint;
            if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, cloexec) != 0 {
                // Before Linux 5.11: one descriptor at a time, up to the limit.
                let mut limit: libc::rlimit = std::mem::zeroed();
                let top = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
                    limit.rlim_cur.min(1 << 20) as libc::c_int
                } else {
                    1024
                };
                for fd in 3..top {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                    }
                }
            }
            Ok(())
        });
    }
    Ok(service)
}

/// Write the pid file whole or not at all: a launch finishing reads it.
fn record_service_pid(directory: &Path, pid: u32) {
    let temporary = directory.join(format!(".{SERVICE_PID}.{}", std::process::id()));
    if std::fs::write(&temporary, pid.to_string()).is_err()
        || std::fs::rename(&temporary, directory.join(SERVICE_PID)).is_err()
    {
        let _ = std::fs::remove_file(temporary);
    }
}

/// Start the service for this socket, preserving other running versions.
fn start_service(
    dirs: &launch::Dirs,
    socket: Option<&Path>,
    cache: &Path,
    plan: &launch::Plan,
) -> Result<()> {
    let socket =
        socket.context("XDG_RUNTIME_DIR or XODUS_RUNTIME_DIR is required for Xbox services")?;
    if service_socket_live(socket) {
        return Ok(());
    }
    let binary = dirs.xodus_service();
    if !binary.is_file() {
        bail!(
            "{} is missing; the runtime's Xbox-side calls go through it",
            binary.display()
        );
    }
    let directory = socket
        .parent()
        .context("invalid service socket directory")?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    // Two launches can both find no service -- Steam and the window at the
    // same moment, or a double-click -- and the service takes seconds to bind.
    // Both would start one, the second would fail to bind, and the pid file
    // could end up naming the one that exited, so the real one is never
    // stopped. One launch starts it; the other then finds it listening.
    let start = directory.join("start.lock");
    let start_lock = open_lock(&start)?;
    flock_or_warn(
        &start_lock,
        libc::LOCK_EX,
        &start,
        "two launches at once may both start a service",
    )?;
    if service_socket_live(socket) {
        return Ok(());
    }
    // Only remove an unreachable socket. An unrelated regular file is an error.
    if let Ok(metadata) = std::fs::symlink_metadata(socket) {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_socket() {
            bail!("{} exists and is not a service socket", socket.display());
        }
        std::fs::remove_file(socket)
            .with_context(|| format!("removing stale {}", socket.display()))?;
    }
    let log = directory.join("xodus-service.log");
    use std::os::unix::fs::OpenOptionsExt;
    let out = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&log)
        .with_context(|| format!("creating {}", log.display()))?;
    let appdir = std::env::var_os("APPDIR").filter(|dir| !dir.is_empty());
    let executable = detached_service(
        &binary,
        appdir.as_deref().map(Path::new),
        &dirs.cli_dir,
        cache,
    )?;
    let spawn = |executable: &Path| {
        let mut service = service_command(executable, directory, &out)?;
        // The pair this launch was resolved for, and nothing inherited.
        for key in [
            "XODUS_STORE_PROFILE",
            "XODUS_PLAYER_PROFILE",
            "XODUS_STORE_PRODUCT",
            "XODUS_STORE_CONTENT",
        ] {
            match plan.get(key) {
                Some(value) => service.env(key, value),
                None => service.env_remove(key),
            };
        }
        service.spawn()
    };
    let mut child = match spawn(&executable) {
        // A cache on a noexec mount: the copy cannot run, the packaged binary
        // can, at the cost the copy exists to avoid.
        Err(error)
            if executable != binary
                && matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM)) =>
        {
            eprintln!(
                "-- cannot run {} ({error}); starting the packaged service, which keeps the AppImage busy until it stops",
                executable.display()
            );
            spawn(&binary).with_context(|| format!("starting {}", binary.display()))?
        }
        spawned => spawned.with_context(|| format!("starting {}", executable.display()))?,
    };
    for _ in 0..100 {
        if service_socket_live(socket) {
            // Recorded only now, and only if the process is still there: the
            // pid file must name the process that owns the socket, and is how
            // the last launch using it finds the service to stop.
            if child
                .try_wait()
                .context("checking service startup")?
                .is_none()
            {
                record_service_pid(directory, child.id());
                eprintln!(":: started xodus-service (log: {})", log.display());
            }
            return Ok(());
        }
        if let Some(status) = child.try_wait().context("checking service startup")? {
            bail!(
                "Xbox service exited {status} before listening; log: {}",
                log.display()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Nothing would ever stop it: it was never recorded.
    let _ = child.kill();
    let _ = child.wait();
    bail!(
        "Xbox service did not start listening; log: {}",
        log.display()
    )
}

fn runtime_socket_from(
    override_dir: Option<std::ffi::OsString>,
    runtime_dir: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    override_dir
        .filter(|d| !d.is_empty())
        .or_else(|| runtime_dir.filter(|d| !d.is_empty()))
        .map(|d| PathBuf::from(d).join("xodus.sock"))
}

fn runtime_socket() -> Option<PathBuf> {
    runtime_socket_from(
        std::env::var_os("XODUS_RUNTIME_DIR"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

fn service_socket_live(socket: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

/// A recipe's `[[setup]]` steps for one moment in the title's life.
///
/// Each action is a named operation this binary implements, never a shell hook:
/// a recipe is data, and data that can run commands is not data. An unknown
/// action stops the launch rather than being skipped, so a recipe written for a
/// newer build says so instead of half-working.
fn run_setup(recipe: &Recipe, when: &str, dirs: &launch::Dirs, game_dir: &Path) -> Result<()> {
    for step in recipe
        .setup
        .iter()
        .filter(|s| s.when.as_deref() == Some(when))
    {
        let dir = match &step.dir {
            Some(sub) => game_dir.join(sub.replace('\\', "/")),
            None => game_dir.to_path_buf(),
        };
        match step.action.as_str() {
            "substitute-xcurl" => {
                let script = dirs.scripts_dir.join("fix-xcurl.sh");
                if !script.is_file() {
                    bail!(
                        "{} is missing, and {} needs it",
                        script.display(),
                        recipe.title.name
                    );
                }
                eprintln!(":: setup: {} ({when})", step.action);
                let mut command = Command::new(script);
                command.arg(&dir);
                let code = run_inherited(command)?;
                if code != 0 {
                    bail!("setup step '{}' failed (exit {code})", step.action);
                }
            }
            other => bail!(
                "{} asks for the setup action '{other}', which this build does not implement",
                recipe.title.name
            ),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal_of(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// Map a child's status onto ours the way a shell does: a signalled child
/// becomes 128 + signal, so a title killed with SIGKILL reports 137 rather than
/// the "no code" `ExitStatus::code()` gives.
fn exit_code_from(code: Option<i32>, signal: Option<i32>) -> u8 {
    if let Some(sig) = signal {
        return u8::try_from(128 + sig).unwrap_or(EXIT_FAILURE);
    }
    match code {
        Some(c) => u8::try_from(c).unwrap_or(EXIT_FAILURE),
        None => EXIT_FAILURE,
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// the capability gate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// Everything the recipe asks for is provided.
    Ready,
    /// Runs, but something optional is unavailable.
    Degraded,
    /// A required capability is absent from a list the runtime published
    /// itself. That is authoritative, so this does not launch.
    Refused,
    /// A required capability is absent from a set *probed* off the files on
    /// disk. Not the same thing: no runtime build publishes a capability list
    /// yet (titles/SCHEMA.md), a probe sees maybe two of the eleven names in
    /// the registry, and refusing on that would block every launch that works
    /// today in order to prevent a failure the machine does not have. So it
    /// warns, loudly, and goes ahead.
    Unverified,
}

fn gate(assessment: &Assessment) -> Gate {
    if assessment.is_satisfied() {
        if assessment.matched.missing_wanted.is_empty() {
            Gate::Ready
        } else {
            Gate::Degraded
        }
    } else if assessment.source.is_published() {
        Gate::Refused
    } else {
        Gate::Unverified
    }
}

/// Why a launch was refused. The explanation comes from the core assessment,
/// which names every missing capability and, with the registry, the symptom
/// each one would have produced -- so nobody has to launch twice to find the
/// second problem.
fn refusal(assessment: &Assessment, runtime: &InstalledRuntime) -> String {
    format!(
        "{}\n   runtime: {}\n   update it (ferestre install-runtime), or launch anyway with --force",
        assessment.explanation.trim_end(),
        runtime.label()
    )
}

/// What to say on stderr before a launch that is going ahead anyway.
fn warnings(recipe: &Recipe, assessment: &Assessment, verdict: Gate, forced: bool) -> Vec<String> {
    let mut lines = Vec::new();
    match verdict {
        Gate::Ready => {}
        Gate::Degraded => lines.push(format!(
            "-- optional and unavailable: {}",
            joined(&assessment.matched.missing_wanted)
        )),
        Gate::Unverified => lines.push(format!(
            "-- this runtime publishes no capability list, so {} could not be checked; launching unverified",
            joined(&assessment.matched.missing_required)
        )),
        Gate::Refused if forced => lines.push(format!(
            "-- --force: launching without {}",
            joined(&assessment.matched.missing_required)
        )),
        Gate::Refused => {}
    }
    if !recipe.status.state.is_runnable() {
        lines.push(format!(
            "-- {} is recorded as {}: {}",
            recipe.title.name,
            recipe.status.state.as_str(),
            recipe.status.summary
        ));
    }
    lines
}

fn joined(names: &BTreeSet<String>) -> String {
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------------------
// account identifiers
// ---------------------------------------------------------------------------

/// Key names that carry account identity, matched exactly (or as the tail of an
/// underscore-separated name, so `XGR_XUID` is caught and `MAX_TOKENS` is not).
/// Short on purpose: it is meant to be read, not to be a filter.
const ACCOUNT_KEYS: &[&str] = &[
    "account_profile",
    "store_profile",
    "player_profile",
    "save_namespace",
    "xuid",
    "puid",
    "gamertag",
    "email",
    "user_id",
    "account_id",
    "token",
    "access_token",
    "refresh_token",
    "licence",
    "license",
    "content_key",
    "ticket",
];

fn is_account_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ACCOUNT_KEYS
        .iter()
        .any(|k| lower == *k || lower.ends_with(&format!("_{k}")))
}

/// Mask an identifier: enough to tell two accounts apart in a bug report, not
/// enough to be one. The shape `tools/xsts_probe.py` already prints.
fn mask(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 4 {
        return "*".repeat(chars.len());
    }
    let mut out = "*".repeat(chars.len() - 4);
    out.extend(chars[chars.len() - 4..].iter());
    out
}

fn env_display(name: &str, value: &str, show_account: bool) -> String {
    if show_account || value.is_empty() || !is_account_key(name) {
        value.to_string()
    } else {
        mask(value)
    }
}

/// Mask account identity anywhere in a JSON document before it is printed.
/// Values are masked rather than removed, so the document keeps its shape for
/// whatever reads it.
fn redact_json(value: &mut Value, show_account: bool) {
    if show_account {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if is_account_key(key) {
                    if let Value::String(s) = v {
                        *s = mask(s);
                        continue;
                    }
                }
                redact_json(v, false);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact_json(v, false)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// One line of `ferestre doctor`. A GUI groups by `section` and shows `hint` when
/// something went wrong; the text renderer does exactly the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Check {
    section: String,
    name: String,
    status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

impl Check {
    fn new(section: &str, name: impl Into<String>, status: CheckStatus) -> Self {
        Check {
            section: section.to_string(),
            name: name.into(),
            status,
            detail: None,
            hint: None,
        }
    }
    fn ok(section: &str, name: impl Into<String>) -> Self {
        Self::new(section, name, CheckStatus::Ok)
    }
    fn warn(section: &str, name: impl Into<String>) -> Self {
        Self::new(section, name, CheckStatus::Warn)
    }
    fn fail(section: &str, name: impl Into<String>) -> Self {
        Self::new(section, name, CheckStatus::Fail)
    }
    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

fn failures(checks: &[Check]) -> usize {
    checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count()
}

/// Everything a launch needs, in the order it would be needed. Same checks the
/// placeholder script made, so an answer people have seen stays the answer.
fn doctor_checks(paths: &Paths) -> Vec<Check> {
    let mut checks = Vec::new();

    checks.push(if std::env::consts::ARCH == "x86_64" {
        Check::ok("host", "x86_64")
    } else {
        Check::fail(
            "host",
            format!(
                "{}: the runtime, Proton and every supported title are x86_64 only",
                std::env::consts::ARCH
            ),
        )
    });
    if let Some(image) = std::env::var_os("APPIMAGE") {
        checks.push(
            Check::ok("host", "running from an AppImage")
                .with_detail(PathBuf::from(image).display().to_string()),
        );
    }

    checks.push(if has_i386_loader(Path::new("/lib/ld-linux.so.2")) {
        Check::ok("supporting", "32-bit Linux loader available")
    } else {
        Check::fail("supporting", "32-bit Linux support is required")
            .with_hint("install the 32-bit compatibility libraries: https://github.com/icex/ferestre/blob/main/docs/DEPENDENCIES.md")
    });

    match paths.xodus_cli().filter(|p| p.is_file()) {
        Some(path) => {
            checks.push(Check::ok("client", "xodus-cli").with_detail(path.display().to_string()))
        }
        None => checks.push(Check::fail("client", "no xodus-cli").with_hint(
            "build it (cargo build --release in the xodus-cli checkout), or set XODUS_CLI_DIR",
        )),
    }
    match paths.xodus_service().filter(|p| p.is_file()) {
        Some(_) => checks.push(Check::ok("client", "xodus-service")),
        None => {
            checks.push(Check::fail("client", "no xodus-service").with_hint(
                "the runtime's Xbox-side calls go through it; it builds beside xodus-cli",
            ))
        }
    }

    match InstalledRuntime::discover(paths) {
        Ok(rt) => {
            checks.push(Check::ok("runtime", "patched Proton").with_detail(rt.label()));
            checks.push(
                match ferestre_core::media::missing_libav_dependencies(
                    &rt.path,
                    paths.steam_dir(),
                    None,
                ) {
                    Ok(missing) if missing.is_empty() => {
                        Check::ok("media", "GStreamer libav dependencies available")
                    }
                    Ok(missing) => Check::warn(
                        "media",
                        "videos may be black or silent: missing media libraries",
                    )
                    .with_detail(missing.join(", "))
                    .with_hint(ferestre_core::media::DEPENDENCY_HINT),
                    Err(error) => {
                        Check::warn("media", "could not check GStreamer libav dependencies")
                            .with_detail(error.to_string())
                    }
                },
            );
            // Spelled out rather than imported: the registry name is the
            // stable thing, and the constant holding it in core is private.
            checks.push(if rt.provides("loader.memfd-main-image") {
                Check::ok("runtime", "keeps inherited file descriptors")
            } else {
                // Without it every GDK title exits 1: Proton's run_proc()
                // closes the fds holding the decrypted executable.
                Check::fail(
                    "runtime",
                    "proton closes inherited fds; every title will exit 1",
                )
                .with_hint("re-run: ferestre install-runtime")
            });
            checks.push(if rt.path.join(runtime::XGAMERUNTIME_DLL).is_file() {
                Check::ok("runtime", "xgameruntime.dll present")
            } else {
                Check::fail("runtime", "no xgameruntime.dll in the compat tool")
                    .with_hint("re-run: ferestre install-runtime")
            });
            checks.push(if rt.source.is_published() {
                Check::ok("runtime", format!("publishes {} capabilities", rt.provides.len()))
            } else {
                Check::warn("runtime", "publishes no capability list")
                    .with_hint("what it provides was probed from the files on disk, so a title's requirements cannot be verified before launch")
            });
            // A warning, not a failure: only PlayStation pads depend on it.
            checks.push(match ferestre_core::sdl::find_sdl2(&rt.path) {
                Ok(Some(path)) => Check::ok("input", "SDL2 available for PlayStation pads")
                    .with_detail(path.display().to_string()),
                Ok(None) => Check::warn(
                    "input",
                    "no SDL2: a DualShock 4 or DualSense will not reach any title",
                )
                .with_detail(ferestre_core::sdl::SDL2_SONAME)
                .with_hint(ferestre_core::sdl::DEPENDENCY_HINT),
                Err(error) => Check::warn("input", "could not check for SDL2")
                    .with_detail(format!("{error:#}")),
            });
        }
        Err(err) => checks.push(
            Check::fail("runtime", "no patched Proton")
                .with_hint(first_line(&err.to_string()))
                .with_detail("run: ferestre install-runtime   (or set XODUS_PROTON_DIR)"),
        ),
    }

    match paths.steam_dir() {
        Some(dir) => checks.push(
            Check::ok("supporting", "Steam install for STEAM_COMPAT_CLIENT_INSTALL_PATH")
                .with_detail(dir.display().to_string()),
        ),
        None => checks.push(
            Check::warn("supporting", "no Steam install found")
                .with_hint("Proton wants STEAM_COMPAT_CLIENT_INSTALL_PATH; set XODUS_STEAM_DIR if yours is somewhere unusual"),
        ),
    }

    let games = paths.games_dir();
    checks.push(match writable(games) {
        Ok(()) => Check::ok("supporting", "games directory writable")
            .with_detail(games.display().to_string()),
        Err(err) => Check::fail(
            "supporting",
            format!("games directory not writable: {}", games.display()),
        )
        .with_hint(err.to_string()),
    });

    checks.push(if vulkan_icd_present() {
        Check::ok("supporting", "a Vulkan driver is installed")
    } else {
        Check::fail(
            "supporting",
            "no Vulkan ICD found; the titles will not render",
        )
        .with_hint("install your GPU vendor's Vulkan driver package")
    });

    // The client stores account tokens in the keyring and aborts outright when
    // there is no Secret Service, rather than degrading.
    checks.push(if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
        Check::ok("supporting", "session D-Bus available for the account keyring")
    } else {
        Check::warn("supporting", "no session D-Bus")
            .with_hint("sign-in needs a Secret Service provider; unlock your keyring, or start gnome-keyring / kwallet")
    });

    match paths.titles_dir() {
        Some(dir) => {
            let count = Recipe::load_dir(dir).map(|r| r.len());
            checks.push(match count {
                Ok(n) => Check::ok("recipes", format!("{n} title recipe(s)"))
                    .with_detail(dir.display().to_string()),
                Err(err) => Check::fail("recipes", "recipes do not parse")
                    .with_hint(first_line(&err.to_string())),
            });
        }
        None => checks.push(
            Check::warn("recipes", "no title recipes found")
                .with_hint("set XODUS_REPO_DIR, or pass --titles-dir"),
        ),
    }

    checks
}

fn has_i386_loader(path: &Path) -> bool {
    path.is_file()
}

/// Writable is tested by writing. Permissions bits lie on NFS, on a full disk,
/// and on a read-only mount.
fn writable(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let probe = dir.join(".ferestre-write-probe");
    std::fs::write(&probe, b"").with_context(|| format!("writing in {}", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

fn vulkan_icd_present() -> bool {
    [
        "/usr/share/vulkan/icd.d",
        "/etc/vulkan/icd.d",
        "/usr/local/share/vulkan/icd.d",
    ]
    .iter()
    .any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|mut entries| {
            entries.any(|e| {
                e.is_ok_and(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            })
        })
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

fn render_checks(checks: &[Check], style: &Style) -> String {
    let mut out = String::new();
    let mut section: Option<&str> = None;
    for c in checks {
        if section != Some(c.section.as_str()) {
            if section.is_some() {
                out.push('\n');
            }
            out.push_str(&format!("== {} ==\n", c.section));
            section = Some(c.section.as_str());
        }
        let (colour, mark) = match c.status {
            CheckStatus::Ok => (style.ok, "ok"),
            CheckStatus::Warn => (style.warn, "--"),
            CheckStatus::Fail => (style.bad, "!!"),
        };
        out.push_str(&format!("{colour} {mark} {}  {}\n", style.off, c.name));
        if let Some(detail) = &c.detail {
            out.push_str(&format!("{}      {detail}{}\n", style.dim, style.off));
        }
        // A hint next to something that is fine is noise.
        if c.status != CheckStatus::Ok {
            if let Some(hint) = &c.hint {
                out.push_str(&format!("{}      {hint}{}\n", style.dim, style.off));
            }
        }
    }
    out
}

/// ANSI codes, or nothing. Colour is decided once, at the edge, so nothing else
/// has to know whether it is writing to a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Style {
    ok: &'static str,
    warn: &'static str,
    bad: &'static str,
    dim: &'static str,
    off: &'static str,
}

impl Style {
    const fn plain() -> Self {
        Style {
            ok: "",
            warn: "",
            bad: "",
            dim: "",
            off: "",
        }
    }

    const fn colour() -> Self {
        Style {
            ok: "\x1b[32m",
            warn: "\x1b[33m",
            bad: "\x1b[31m",
            dim: "\x1b[2m",
            off: "\x1b[0m",
        }
    }

    fn detect(is_terminal: bool, no_color_flag: bool) -> Self {
        if no_color_flag || std::env::var_os("NO_COLOR").is_some() || !is_terminal {
            Style::plain()
        } else {
            Style::colour()
        }
    }
}

/// What `ferestre titles --json` publishes. Built by hand rather than by deriving
/// Serialize on Recipe, so adding a field to the recipe schema cannot silently
/// change a document a GUI is parsing.
#[derive(Debug, Serialize)]
struct TitleRow<'a> {
    product_id: &'a str,
    name: &'a str,
    slug: &'a str,
    publisher: Option<&'a str>,
    state: &'a str,
    runnable: bool,
    summary: &'a str,
    stops_at: Option<&'a str>,
    blocked_by: Option<&'a str>,
    last_verified: Option<String>,
    install_size_gb: Option<f64>,
    requires: Vec<&'a str>,
    wants: Vec<&'a str>,
    install_dir: &'a str,
    prefix_dir: String,
    issues: usize,
}

impl<'a> From<&'a Recipe> for TitleRow<'a> {
    fn from(r: &'a Recipe) -> Self {
        TitleRow {
            product_id: &r.title.product_id,
            name: &r.title.name,
            slug: &r.title.slug,
            publisher: r.title.publisher.as_deref(),
            state: r.status.state.as_str(),
            runnable: r.status.state.is_runnable(),
            summary: &r.status.summary,
            stops_at: r.status.stops_at.as_deref(),
            blocked_by: r.status.blocked_by.as_deref(),
            // Datetime renders itself; naming its type would pull toml into
            // this crate for one field.
            last_verified: r.status.last_verified.as_ref().map(|d| d.to_string()),
            install_size_gb: r.title.install_size_gb,
            requires: r.runtime.requires.iter().map(String::as_str).collect(),
            wants: r.runtime.wants.iter().map(String::as_str).collect(),
            install_dir: r.install_dir(),
            prefix_dir: r.prefix_dir(),
            issues: r.issues.len(),
        }
    }
}

fn titles_table(recipes: &[Recipe]) -> Vec<Vec<String>> {
    let mut rows = vec![vec![
        "PRODUCT ID".into(),
        "SLUG".into(),
        "STATE".into(),
        "TITLE".into(),
    ]];
    for r in recipes {
        rows.push(vec![
            r.title.product_id.clone(),
            r.title.slug.clone(),
            r.status.state.as_str().to_string(),
            r.title.name.clone(),
        ]);
    }
    rows
}

/// The text form of a library listing.
///
/// Only these fields are ever read, and that is how "no account identity" is
/// enforced for the text output: something the renderer does not ask for cannot
/// reach the terminal, whatever the client puts in the document.
fn library_table(rows: &[&library::Row]) -> Vec<Vec<String>> {
    let mut out = vec![vec![
        "PRODUCT ID".into(),
        "TITLE".into(),
        "OWNERSHIP".into(),
        "STATE".into(),
    ]];
    for row in rows {
        out.push(vec![
            row.product_id.to_string(),
            row.name().to_string(),
            row.standing().as_str().to_string(),
            row.recipe
                .map(|r| r.status.state.as_str())
                .unwrap_or("-")
                .to_string(),
        ]);
    }
    out
}

/// The `--json` document for `ferestre library`.
///
/// A subset of `docs/CATALOG.md` §7.1: the fields there that come from the
/// catalog join (display name, artwork, package family name, sizes) need a call
/// nothing makes yet, so they are absent rather than invented.
fn library_json(rows: &[&library::Row], skipped: &[(String, &'static str)]) -> Value {
    let titles: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "product_id": row.product_id,
                "name": row.name(),
                "standing": row.standing().as_str(),
                "launchable": row.is_launchable(),
                "entitlement": row.entry.map(|e| json!({
                    "status": e.status.as_str(),
                    "product_type": e.product_type.as_str(),
                    "sku_type": e.sku_type,
                    "ownership_type": e.ownership_type,
                    "acquired": e.acquired_date,
                })),
                "recipe": row.recipe.map(|r| json!({
                    "slug": r.title.slug,
                    "state": r.status.state.as_str(),
                    "runnable": r.status.state.is_runnable(),
                    "summary": r.status.summary,
                })),
            })
        })
        .collect();
    json!({
        "schema": JSON_SCHEMA,
        "source": "collections",
        "stale": false,
        "titles": titles,
        "skipped": skipped.iter()
            .map(|(id, why)| json!({ "product_id": id, "reason": why }))
            .collect::<Vec<_>>(),
    })
}

fn plan_json(plan: &launch::Plan) -> Value {
    json!({
        "program": plan.program.display().to_string(),
        "args": plan.args,
        "env_set": plan.env_set,
        "env_unset": plan.env_unset,
        "cwd": plan.cwd.as_ref().map(|p| p.display().to_string()),
    })
}

/// Pad columns to a common width. The last column is never padded, so no line
/// carries trailing spaces into a paste or a diff.
fn table(rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = Vec::new();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let w = cell.chars().count();
            match widths.get_mut(i) {
                Some(existing) => *existing = (*existing).max(w),
                None => widths.push(w),
            }
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            line.push_str(cell);
            if i + 1 < row.len() {
                for _ in cell.chars().count()..widths[i] {
                    line.push(' ');
                }
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{prefix}{l}")
            }
        })
        .map(|l| l + "\n")
        .collect()
}

/// `--dry-run`: the launch written so it can be pasted into a shell and run.
/// That is the point -- someone debugging a launch needs the exact command, not
/// a description of it.
fn render_plan(plan: &launch::Plan) -> String {
    let mut out = String::new();
    for name in &plan.env_unset {
        out.push_str(&format!("unset {name}\n"));
    }
    for (name, value) in &plan.env_set {
        out.push_str(&format!("export {name}={}\n", shell_quote(value)));
    }
    if let Some(cwd) = &plan.cwd {
        out.push_str(&format!("cd {}\n", shell_quote(&cwd.display().to_string())));
    }
    out.push_str(&shell_quote(&plan.program.display().to_string()));
    for arg in &plan.args {
        out.push(' ');
        out.push_str(&shell_quote(arg));
    }
    out.push('\n');
    out
}

/// Quote for `sh`. Single quotes, because they are literal, with the usual
/// `'\''` dance for an embedded quote.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

// ---------------------------------------------------------------------------
// odds and ends
// ---------------------------------------------------------------------------

/// A Store product id: 12 characters, letters and digits. Checked before it
/// reaches the client, so a stray `--flag` or a path cannot be mistaken for one
/// and so the failure names the format instead of coming back from a network
/// call two minutes later.
fn is_product_id(s: &str) -> bool {
    s.len() == 12 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

fn normalise_product_id(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// An error that means "you typed the command wrong", which exits 2 like
/// clap's own rather than 1.
fn usage(message: &str) -> anyhow::Error {
    UsageError(message.to_string()).into()
}

#[derive(Debug)]
struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UsageError {}

fn print_json(value: &Value) {
    // Pretty, because a person reads this as often as a program does and jq is
    // not always installed.
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("json value serialises")
    );
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory and the children a test started in it. Both go
    /// when the test ends, whether or not an assertion failed first.
    struct Scratch {
        root: PathBuf,
        children: Vec<std::process::Child>,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("ferestre-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self {
                root,
                children: Vec::new(),
            }
        }

        /// Start this test binary as a stand-in process the kernel names
        /// `name`, working in `directory`. It runs `stand_in` and sleeps until
        /// killed. Nothing on the host is needed: a copy of `sleep` is not
        /// sleep everywhere -- on a multicall coreutils or busybox, it is a
        /// binary that dispatches on its own name.
        fn stand_in(&mut self, name: &str, directory: &Path, ignore_term: bool) -> usize {
            let bin = self.root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let link = bin.join(name);
            if !link.exists() {
                std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
            }
            let ready = self.root.join(format!("ready-{}", self.children.len()));
            let child = Command::new(&link)
                .args([
                    "--ignored",
                    "--exact",
                    "tests::stand_in",
                    "--test-threads=1",
                ])
                .env("FERESTRE_STAND_IN_READY", &ready)
                .env(
                    "FERESTRE_STAND_IN",
                    if ignore_term { "ignore-term" } else { "sleep" },
                )
                .current_dir(directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            self.children.push(child);
            // Ready means the stand-in has settled what it does with SIGTERM.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !ready.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "stand-in never started"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            self.children.len() - 1
        }

        fn pid(&self, child: usize) -> i32 {
            self.children[child].id() as i32
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            for child in &mut self.children {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Not a test: the process the service tests start in place of the
    /// service. Run on its own it returns at once.
    #[test]
    #[ignore = "started by the service tests as a stand-in process"]
    fn stand_in() {
        let Some(mode) = std::env::var_os("FERESTRE_STAND_IN") else {
            return;
        };
        if mode == "ignore-term" {
            // Safety: ignoring a signal, in a process doing nothing else.
            unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
        }
        if let Some(ready) = std::env::var_os("FERESTRE_STAND_IN_READY") {
            std::fs::write(ready, b"").unwrap();
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// Steam shows a shortcut as running until every process it started has
    /// gone, so the service must end with the last title using it -- and not
    /// before.
    #[test]
    fn the_last_title_to_finish_stops_the_service() {
        let mut scratch = Scratch::new("service-use");
        let directory = scratch.root.join("service");
        std::fs::create_dir_all(&directory).unwrap();
        let service = scratch.stand_in("xodus-service", &directory, false);
        std::fs::write(
            directory.join(SERVICE_PID),
            scratch.pid(service).to_string(),
        )
        .unwrap();
        let socket = directory.join("xodus.sock");

        let first = ServiceUse::begin(Some(&socket)).unwrap().unwrap();
        let second = ServiceUse::begin(Some(&socket)).unwrap().unwrap();
        drop(first);
        assert!(
            scratch.children[service].try_wait().unwrap().is_none(),
            "still in use by the second title"
        );
        drop(second);
        let status = scratch.children[service].wait().unwrap();
        assert_eq!(signal_of(&status), Some(libc::SIGTERM), "stopped: {status}");
        assert!(!directory.join(SERVICE_PID).exists());

        // An unrelated process behind a stale pid is never signalled.
        let unrelated = scratch.stand_in("unrelated", &directory, false);
        std::fs::write(
            directory.join(SERVICE_PID),
            scratch.pid(unrelated).to_string(),
        )
        .unwrap();
        drop(ServiceUse::begin(Some(&socket)).unwrap().unwrap());
        assert!(scratch.children[unrelated].try_wait().unwrap().is_none());
    }

    /// Every version's service is named xodus-service. A stale pid that now
    /// belongs to another directory's service must not stop it.
    #[test]
    fn another_directorys_service_behind_a_stale_pid_is_left_alone() {
        let mut scratch = Scratch::new("service-other");
        let directory = scratch.root.join("service");
        let elsewhere = scratch.root.join("other-version");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let other = scratch.stand_in("xodus-service", &elsewhere, false);
        std::fs::write(directory.join(SERVICE_PID), scratch.pid(other).to_string()).unwrap();
        stop_service(&directory);
        assert!(scratch.children[other].try_wait().unwrap().is_none());
        assert!(!directory.join(SERVICE_PID).exists());
    }

    /// A launch waiting for the lock must not find a service that is being
    /// killed still listening.
    #[test]
    fn a_service_that_ignores_sigterm_is_gone_before_the_lock_is_let_go() {
        let mut scratch = Scratch::new("service-kill");
        let directory = scratch.root.join("service");
        std::fs::create_dir_all(&directory).unwrap();
        let service = scratch.stand_in("xodus-service", &directory, true);
        let pid = scratch.pid(service);
        std::fs::write(directory.join(SERVICE_PID), pid.to_string()).unwrap();
        drop(
            ServiceUse::begin(Some(&directory.join("xodus.sock")))
                .unwrap()
                .unwrap(),
        );
        // Exited by the time the lock was dropped: a zombie, waiting for us.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            stat.is_empty()
                || stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, s)| s.starts_with('Z')),
            "still running: {stat}"
        );
        let status = scratch.children[service].wait().unwrap();
        assert_eq!(signal_of(&status), Some(libc::SIGKILL), "{status}");
    }

    /// std learns that exec failed through a pipe the child inherits. Closed
    /// before exec, a missing service looked like a service that started and
    /// aborted.
    #[test]
    fn a_service_that_cannot_be_executed_reports_why() {
        let scratch = Scratch::new("service-exec");
        let log = std::fs::File::create(scratch.root.join("log")).unwrap();
        let spawn = |executable: &Path| {
            service_command(executable, &scratch.root, &log)
                .unwrap()
                .spawn()
                .map(|mut child| {
                    let _ = child.kill();
                    let _ = child.wait();
                })
        };
        let missing = spawn(&scratch.root.join("absent"));
        assert_eq!(missing.unwrap_err().kind(), std::io::ErrorKind::NotFound);
        let inert = scratch.root.join("xodus-service");
        std::fs::write(&inert, b"not a program").unwrap();
        assert_eq!(
            spawn(&inert).unwrap_err().raw_os_error(),
            Some(libc::EACCES)
        );
    }

    /// A window stopping a title signals its process group; a shared service
    /// must not be in it.
    #[test]
    fn the_service_runs_in_its_own_process_group() {
        let mut scratch = Scratch::new("service-group");
        let log = std::fs::File::create(scratch.root.join("log")).unwrap();
        let bin = scratch.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join("xodus-service");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
        let child = service_command(&link, &scratch.root, &log)
            .unwrap()
            .args(["--ignored", "--exact", "tests::stand_in"])
            .env("FERESTRE_STAND_IN", "sleep")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        scratch.children.push(child);
        // Safety: getpgid only reads.
        assert_eq!(unsafe { libc::getpgid(pid) }, pid);
        assert_ne!(unsafe { libc::getpgid(0) }, pid);
    }

    /// A service started from the AppImage pinned its mount, so the AppImage
    /// Steam launched never exited. It runs from a copy keyed by SERVICE_ID.
    #[test]
    fn a_service_inside_an_appimage_runs_from_a_copy_outside_it() {
        use std::os::unix::fs::MetadataExt;
        let scratch = Scratch::new("service-copy");
        let root = &scratch.root;
        let appdir = root.join("mount");
        let cli = appdir.join("usr/lib/ferestre/client");
        let cache = root.join("cache");
        std::fs::create_dir_all(&cli).unwrap();
        let binary = cli.join("xodus-service");
        std::fs::write(&binary, b"fixture service").unwrap();
        let id = "c".repeat(64);
        std::fs::write(cli.join("SERVICE_ID"), format!("{id}\n")).unwrap();

        let copy = detached_service(&binary, Some(&appdir), &cli, &cache).unwrap();
        assert_eq!(copy, cache.join("services").join(&id).join("xodus-service"));
        assert_eq!(std::fs::read(&copy).unwrap(), b"fixture service");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o755
        );

        // Reused, not copied again, while it matches: the same file, with
        // bytes the package never had.
        std::fs::write(&copy, b"fixture SERVICE").unwrap();
        let before = std::fs::metadata(&copy).unwrap();
        assert_eq!(
            detached_service(&binary, Some(&appdir), &cli, &cache).unwrap(),
            copy
        );
        let after = std::fs::metadata(&copy).unwrap();
        assert_eq!(
            (after.ino(), after.mtime(), after.mtime_nsec()),
            (before.ino(), before.mtime(), before.mtime_nsec())
        );
        assert_eq!(std::fs::read(&copy).unwrap(), b"fixture SERVICE");
        // One that does not match is replaced.
        std::fs::write(&copy, b"truncated").unwrap();
        assert_eq!(
            detached_service(&binary, Some(&appdir), &cli, &cache).unwrap(),
            copy
        );
        assert_eq!(std::fs::read(&copy).unwrap(), b"fixture service");

        // Native installs keep the packaged binary.
        assert_eq!(
            detached_service(&binary, None, &cli, &cache).unwrap(),
            binary
        );
        // So do packages with no id, or one that is not a SHA-256, and they
        // leave nothing in the cache.
        let untouched = root.join("untouched-cache");
        std::fs::remove_file(cli.join("SERVICE_ID")).unwrap();
        for id in [None, Some("zz".to_string()), Some("d".repeat(63))] {
            if let Some(id) = &id {
                std::fs::write(cli.join("SERVICE_ID"), id).unwrap();
            }
            assert_eq!(
                detached_service(&binary, Some(&appdir), &cli, &untouched).unwrap(),
                binary,
                "{id:?}"
            );
            assert!(!untouched.exists(), "{id:?}");
        }
    }

    /// Each update that changes the service leaves a full-size copy behind.
    #[test]
    fn copies_of_other_versions_go_once_nothing_runs_them() {
        let mut scratch = Scratch::new("service-cleanup");
        let services = scratch.root.join("services");
        let make = |id: &str, age: u64| {
            let directory = services.join(id);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("xodus-service"), b"old").unwrap();
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::open(&directory)
                .unwrap()
                .set_modified(when)
                .unwrap();
            std::fs::canonicalize(directory).unwrap()
        };
        let current = "c".repeat(64);
        let kept = make(&current, 3600);
        let stale = make(&"a".repeat(64), 3600);
        let fresh = make(&"b".repeat(64), 0);
        let running = make(&"e".repeat(64), 3600);
        let unknown = make("not-a-service", 3600);
        remove_other_versions(&services, &current, &[running.join("xodus-service")]);
        assert!(!stale.exists());
        for directory in [&kept, &fresh, &running, &unknown] {
            assert!(directory.exists(), "{}", directory.display());
        }

        // And half-written copies whose launch has gone.
        let live = scratch.stand_in("copying", &scratch.root.clone(), false);
        let live = kept.join(format!(".xodus-service.{}", scratch.pid(live)));
        let dead = kept.join(".xodus-service.4000000000");
        std::fs::write(&live, b"").unwrap();
        std::fs::write(&dead, b"").unwrap();
        remove_abandoned_copies(&kept);
        assert!(live.exists());
        assert!(!dead.exists());

        // What is running is found by its executable.
        let me = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert!(running_executables().contains(&me));
    }

    /// `wineserver -w` waits for every process in the session. One that was
    /// there before the launch may be the title itself, still running.
    #[test]
    fn a_running_wineserver_is_seen_by_its_lock() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        let scratch = Scratch::new("wineserver");
        let base = scratch.root.join("wine");
        let prefix = scratch.root.join("pfx");
        std::fs::create_dir_all(&prefix).unwrap();
        assert_eq!(wineserver_running_in(&base, &prefix), Some(false));

        let metadata = std::fs::metadata(&prefix).unwrap();
        let server = base.join(format!("server-{:x}-{:x}", metadata.dev(), metadata.ino()));
        std::fs::create_dir_all(&server).unwrap();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(server.join("lock"))
            .unwrap();
        assert_eq!(wineserver_running_in(&base, &prefix), Some(false));
        // Held the way wineserver holds it, but as an open file description
        // lock: a process never conflicts with its own ordinary locks.
        let mut held: libc::flock = unsafe { std::mem::zeroed() };
        held.l_type = libc::F_WRLCK as _;
        held.l_whence = libc::SEEK_SET as _;
        held.l_len = 1;
        assert_eq!(
            unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_OFD_SETLK, &held) },
            0
        );
        assert_eq!(wineserver_running_in(&base, &prefix), Some(true));
        assert_eq!(
            wineserver_running_in(&base, &scratch.root.join("gone")),
            None
        );
    }

    #[test]
    fn a_wait_that_takes_too_long_is_abandoned_and_its_child_reaped() {
        let mut scratch = Scratch::new("bounded-wait");
        let child = scratch.stand_in("waiter", &scratch.root.clone(), false);
        let started = std::time::Instant::now();
        assert!(!wait_at_most(
            &mut scratch.children[child],
            std::time::Duration::from_millis(300)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // Already reaped: there is nothing left to wait for.
        assert!(scratch.children[child]
            .try_wait()
            .is_ok_and(|s| s.is_some()));
    }

    /// Stopped from the window or with Ctrl+C, the launch passes the signal to
    /// its title and finishes normally, so its claim on the service drops.
    #[test]
    fn a_signal_to_the_launch_goes_to_the_title() {
        let mut scratch = Scratch::new("forwarding");
        let forwarding = Forwarding::install().unwrap();
        let title = scratch.stand_in("title", &scratch.root.clone(), false);
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            // Safety: SIGTERM to ourselves, which the handler now catches.
            unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
        });
        let status = forwarding.wait(&mut scratch.children[title]).unwrap();
        assert_eq!(signal_of(&status), Some(libc::SIGTERM));
        assert_eq!(forwarding.received(), Some(libc::SIGTERM));
        // Ctrl+C should still stop the rest of the test run.
        for signal in LAUNCH_SIGNALS {
            // Safety: restoring the default disposition.
            unsafe { libc::signal(signal, libc::SIG_DFL) };
        }
    }

    #[test]
    fn packaged_service_versions_are_isolated_only_for_capable_runtimes() {
        let root = Path::new("/desktop");
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        assert_ne!(
            scoped_service_directory(true, &first, root).unwrap(),
            scoped_service_directory(true, &second, root).unwrap()
        );
        assert_eq!(scoped_service_directory(false, &first, root).unwrap(), None);
        assert_eq!(
            scoped_service_directory(false, "legacy metadata", root).unwrap(),
            None
        );
        assert!(scoped_service_directory(true, "../other", root).is_err());
        let socket = scoped_service_directory(true, &first, Path::new("/run/user/1000"))
            .unwrap()
            .unwrap()
            .join("xodus.sock");
        assert!(
            socket.as_os_str().len() < 108,
            "service namespace must fit a Linux Unix socket path"
        );
    }

    #[test]
    fn service_directory_override_preserves_legacy_default() {
        assert_eq!(
            runtime_socket_from(Some("/candidate".into()), Some("/desktop".into())),
            Some(PathBuf::from("/candidate/xodus.sock"))
        );
        assert_eq!(
            runtime_socket_from(Some("".into()), Some("/desktop".into())),
            Some(PathBuf::from("/desktop/xodus.sock"))
        );
        assert_eq!(
            runtime_socket_from(None, Some("/desktop".into())),
            Some(PathBuf::from("/desktop/xodus.sock"))
        );
        assert_eq!(runtime_socket_from(None, None), None);
    }

    #[test]
    fn service_readiness_is_scoped_to_the_requested_socket() {
        let directory =
            std::env::temp_dir().join(format!("ferestre-service-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let first = directory.join("first.sock");
        let other = directory.join("other.sock");
        let listener = std::os::unix::net::UnixListener::bind(&first).unwrap();
        assert!(service_socket_live(&first));
        assert!(!service_socket_live(&other));
        drop(listener);
        // A stale path is not a running service. Other tests start children
        // meanwhile, and one forked but not yet exec'd still holds a copy of
        // the listener for a moment, so give it that moment.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while service_socket_live(&first) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!service_socket_live(&first));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn update_command_asks_the_client_for_json() {
        let command = update_command(Path::new("/opt/xodus/xodus-cli"), "content-id");
        assert_eq!(command.get_program(), Path::new("/opt/xodus/xodus-cli"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["update", "content-id", "--json"]
        );
    }
    use clap::CommandFactory;
    use ferestre_core::capability::Match;
    use ferestre_core::runtime::CapabilitySource;

    const RECIPE: &str = r#"
schema = 1
[title]
product-id = "9NBLGGH2JHXJ"
name = "Minecraft for Windows"
slug = "bedrock"
[launch]
executable = 'Minecraft.Windows.exe'
[runtime]
requires = ["loader.memfd-main-image", "gameinput.v2"]
wants = ["winhttp.websocket-close-timeout"]
[status]
state = "playable"
summary = "Runs."
"#;

    fn recipe() -> Recipe {
        Recipe::parse(RECIPE).expect("fixture parses")
    }

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn assessment(
        missing_required: &[&str],
        missing_wanted: &[&str],
        source: CapabilitySource,
    ) -> Assessment {
        Assessment {
            matched: Match {
                missing_required: names(missing_required),
                missing_wanted: names(missing_wanted),
            },
            source,
            explanation: "explanation from the core assessment".to_string(),
        }
    }

    // -- argument parsing ---------------------------------------------------

    #[test]
    fn the_command_line_is_well_formed() {
        // clap's own verifier catches the mistakes that otherwise only surface
        // as a panic on someone else's machine: a duplicate short flag, a
        // positional after a variadic, an alias shadowing a command.
        Cli::command().debug_assert();
    }

    #[test]
    fn every_subcommand_the_placeholder_had_still_parses() {
        // These names are the contract: the placeholder shipped with them and
        // people have desktop files and scripts using them.
        for args in [
            vec!["ferestre", "doctor"],
            vec!["ferestre", "titles"],
            vec!["ferestre", "library"],
            vec!["ferestre", "run", "bedrock"],
            vec!["ferestre", "install", "9NBLGGH2JHXJ"],
            vec!["ferestre", "install-runtime"],
            vec!["ferestre", "inspect-package", "/tmp/example.appx"],
            vec!["ferestre", "env"],
            vec!["ferestre", "version"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
    }

    #[test]
    fn download_is_still_accepted_as_a_name_for_install() {
        let cli = Cli::try_parse_from(["ferestre", "download", "9NBLGGH2JHXJ"]).unwrap();
        assert!(matches!(cli.command, Cmd::Install { .. }));
    }

    #[test]
    fn installing_an_existing_destination_uses_the_incremental_streaming_path() {
        let command = streaming_command(
            Path::new("/opt/xodus/xodus-cli"),
            "9NBLGGH2JHXJ",
            Path::new("/games/bedrock"),
        );
        assert_eq!(command.get_program(), Path::new("/opt/xodus/xodus-cli"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["streaming", "9NBLGGH2JHXJ", "/games/bedrock"]
        );
    }

    #[test]
    fn json_works_on_either_side_of_the_subcommand() {
        assert!(
            Cli::try_parse_from(["ferestre", "--json", "titles"])
                .unwrap()
                .json
        );
        assert!(
            Cli::try_parse_from(["ferestre", "titles", "--json"])
                .unwrap()
                .json
        );
    }

    #[test]
    fn a_titles_arguments_are_never_parsed_as_ours() {
        let cli =
            Cli::try_parse_from(["ferestre", "run", "bedrock", "--", "--json", "-x"]).unwrap();
        match cli.command {
            Cmd::Run { title, args, .. } => {
                assert_eq!(title, "bedrock");
                assert_eq!(args, vec!["--json", "-x"]);
            }
            other => panic!("expected run, got {other:?}"),
        }
        // ...and the global --json is not set by the title's own --json.
        assert!(!cli.json);
    }

    #[test]
    fn a_missing_or_unknown_subcommand_is_a_usage_error() {
        assert_eq!(
            Cli::try_parse_from(["ferestre"]).unwrap_err().exit_code(),
            EXIT_USAGE as i32
        );
        assert_eq!(
            Cli::try_parse_from(["ferestre", "frobnicate"])
                .unwrap_err()
                .exit_code(),
            EXIT_USAGE as i32
        );
    }

    #[test]
    fn a_usage_error_from_us_exits_2_and_everything_else_exits_1() {
        assert_eq!(exit_code_for(&usage("wrong")), EXIT_USAGE);
        assert_eq!(exit_code_for(&anyhow!("the disk is full")), EXIT_FAILURE);
    }

    // -- the capability gate ------------------------------------------------

    #[test]
    fn a_runtime_with_everything_is_ready() {
        let a = assessment(&[], &[], CapabilitySource::Manifest);
        assert_eq!(gate(&a), Gate::Ready);
        assert!(warnings(&recipe(), &a, Gate::Ready, false).is_empty());
    }

    #[test]
    fn a_published_list_missing_a_requirement_refuses() {
        let a = assessment(&["gameinput.v2"], &[], CapabilitySource::Manifest);
        assert_eq!(gate(&a), Gate::Refused);
        let rt = InstalledRuntime {
            path: PathBuf::from("/opt/xodus"),
            version: Some("xodus-11.0".into()),
            provides: names(&["loader.memfd-main-image"]),
            source: CapabilitySource::Manifest,
        };
        let message = refusal(&a, &rt);
        assert!(
            message.contains("explanation from the core assessment"),
            "{message}"
        );
        assert!(message.contains("xodus-11.0"), "{message}");
        assert!(message.contains("--force"), "{message}");
    }

    #[test]
    fn a_probed_set_missing_a_requirement_warns_instead_of_refusing() {
        // This is the difference that decides whether anything launches today:
        // no runtime build publishes a capability list yet, and a probe sees a
        // fraction of the names a recipe asks for. Refusing on a probed miss
        // would block every working install.
        let a = assessment(&["gameinput.v2"], &[], CapabilitySource::Probed);
        assert_eq!(gate(&a), Gate::Unverified);
        let said = warnings(&recipe(), &a, Gate::Unverified, false).join("\n");
        assert!(said.contains("gameinput.v2"), "{said}");
        assert!(said.contains("no capability list"), "{said}");
    }

    #[test]
    fn a_missing_want_degrades_but_does_not_refuse() {
        let a = assessment(
            &[],
            &["winhttp.websocket-close-timeout"],
            CapabilitySource::List,
        );
        assert_eq!(gate(&a), Gate::Degraded);
        let said = warnings(&recipe(), &a, Gate::Degraded, false).join("\n");
        assert!(said.contains("winhttp.websocket-close-timeout"), "{said}");
    }

    #[test]
    fn forcing_past_a_refusal_still_says_what_is_missing() {
        let a = assessment(&["gameinput.v2"], &[], CapabilitySource::Manifest);
        let said = warnings(&recipe(), &a, Gate::Refused, true).join("\n");
        assert!(said.contains("--force"), "{said}");
        assert!(said.contains("gameinput.v2"), "{said}");
        // Without --force nothing is printed, because nothing is launched.
        assert!(warnings(&recipe(), &a, Gate::Refused, false).is_empty());
    }

    #[test]
    fn a_title_that_is_not_runnable_warns_even_on_a_perfect_runtime() {
        let text = RECIPE
            .replace("state = \"playable\"", "state = \"broken\"")
            .replace(
                "summary = \"Runs.\"",
                "summary = \"In-binary protection.\"\nblocked-by = \"title-protection\"",
            );
        let broken = Recipe::parse(&text).unwrap();
        let a = assessment(&[], &[], CapabilitySource::Manifest);
        let said = warnings(&broken, &a, Gate::Ready, false).join("\n");
        assert!(said.contains("broken"), "{said}");
        assert!(said.contains("In-binary protection."), "{said}");
    }

    // -- account identifiers ------------------------------------------------

    #[test]
    fn an_xuid_is_masked_by_default_and_shown_on_request() {
        let xuid = "2533274812345678";
        assert_eq!(env_display("XGR_XUID", xuid, false), "************5678");
        assert_eq!(env_display("XGR_XUID", xuid, true), xuid);
        // A path is not an identifier and must survive intact.
        assert_eq!(
            env_display("XODUS_GAMES_DIR", "/home/someone/xbox-games", false),
            "/home/someone/xbox-games"
        );
    }

    #[test]
    fn masking_never_leaves_a_short_value_whole() {
        assert_eq!(mask("abcd"), "****");
        assert_eq!(mask("a"), "*");
        assert_eq!(mask(""), "");
        assert_eq!(mask("abcde"), "*bcde");
    }

    #[test]
    fn account_fields_anywhere_in_a_document_are_masked() {
        let mut doc = json!({
            "titles": [{
                "product_id": "9NBLGGH2JHXJ",
                "name": "Minecraft for Windows",
                "gamertag": "SomePlayer123",
                "entitlement": { "status": "Active", "xuid": "2533274812345678" }
            }],
            "market": "US"
        });
        redact_json(&mut doc, false);
        let text = doc.to_string();
        assert!(!text.contains("SomePlayer123"), "{text}");
        assert!(!text.contains("2533274812345678"), "{text}");
        // Everything else is untouched.
        assert!(text.contains("9NBLGGH2JHXJ"), "{text}");
        assert!(text.contains("US"), "{text}");
    }

    #[test]
    fn show_account_leaves_a_document_alone() {
        let mut doc = json!({ "gamertag": "SomePlayer123" });
        redact_json(&mut doc, true);
        assert_eq!(doc["gamertag"], "SomePlayer123");
    }

    #[test]
    fn a_key_that_merely_ends_in_a_word_is_not_mangled() {
        assert!(is_account_key("XGR_XUID"));
        assert!(is_account_key("access_token"));
        assert!(!is_account_key("XODUS_GAMES_DIR"));
        assert!(!is_account_key("tokenizer"));
        assert!(!is_account_key("license_dir"));
    }

    // -- product ids --------------------------------------------------------

    #[test]
    fn product_ids_are_checked_before_they_reach_the_client() {
        assert!(is_product_id("9NBLGGH2JHXJ"));
        assert!(is_product_id("9nblggh2jhxj"));
        assert!(!is_product_id("bedrock"));
        assert!(!is_product_id("9NBLGGH2JHX"));
        assert!(!is_product_id("../../etc/passwd"));
        assert!(!is_product_id("--refresh"));
        assert_eq!(normalise_product_id("9nblggh2jhxj"), "9NBLGGH2JHXJ");
    }

    // -- formatting ---------------------------------------------------------

    #[test]
    fn columns_line_up_and_no_line_has_trailing_space() {
        let out = table(&[
            vec!["PRODUCT ID".into(), "SLUG".into(), "TITLE".into()],
            vec![
                "9NBLGGH2JHXJ".into(),
                "bedrock".into(),
                "Minecraft for Windows".into(),
            ],
            vec![
                "9PPT8K6GQHRZ".into(),
                "fh5".into(),
                "Forza Horizon 5".into(),
            ],
        ]);
        let lines: Vec<&str> = out.lines().collect();
        let slug_at = lines[0].find("SLUG").unwrap();
        assert_eq!(lines[1].find("bedrock"), Some(slug_at));
        assert_eq!(lines[2].find("fh5"), Some(slug_at));
        for l in &lines {
            assert_eq!(*l, l.trim_end(), "trailing whitespace in {l:?}");
        }
    }

    #[test]
    fn a_ragged_row_does_not_panic() {
        // Rows are built by hand in several places; a short one must not take
        // the whole command down.
        assert_eq!(
            table(&[vec!["a".into()], vec!["bb".into(), "cc".into()]]),
            "a\nbb  cc\n"
        );
    }

    #[test]
    fn the_titles_table_leads_with_the_id_people_have() {
        let rows = titles_table(&[recipe()]);
        assert_eq!(rows[0][0], "PRODUCT ID");
        assert_eq!(rows[1][0], "9NBLGGH2JHXJ");
        assert_eq!(rows[1][1], "bedrock");
        assert_eq!(rows[1][2], "playable");
    }

    #[test]
    fn the_json_row_exposes_what_a_gui_needs() {
        let v = serde_json::to_value(TitleRow::from(&recipe())).unwrap();
        assert_eq!(v["product_id"], "9NBLGGH2JHXJ");
        assert_eq!(v["state"], "playable");
        assert_eq!(v["runnable"], true);
        assert_eq!(v["install_dir"], "bedrock");
        assert_eq!(v["prefix_dir"], "bedrock-proton");
        assert_eq!(v["requires"][0], "loader.memfd-main-image");
        assert_eq!(v["issues"], 0);
    }

    #[test]
    fn the_library_renderer_prints_only_fields_it_asked_for() {
        // The renderer is the enforcement: a field it does not read cannot
        // reach the terminal, whatever the client puts in the listing.
        let entries = library::parse(
            r#"{"items":[{"productId":"9NBLGGH2JHXJ","productType":"Game","status":"Active","gamertag":"SomePlayer123"}]}"#,
        )
        .unwrap();
        let recipes = vec![recipe()];
        let rows = library::join(&entries, &recipes);
        let refs: Vec<&library::Row> = rows.iter().collect();
        let out = table(&library_table(&refs));
        assert!(out.contains("9NBLGGH2JHXJ"), "{out}");
        assert!(out.contains("Minecraft for Windows"), "{out}");
        assert!(out.contains("owned-with-recipe"), "{out}");
        assert!(!out.contains("SomePlayer123"), "{out}");
    }

    #[test]
    fn the_library_document_says_why_a_row_was_filtered_out() {
        let entries = library::parse(
            r#"{"items":[
                 {"productId":"9NBLGGH2JHXJ","productType":"Game","status":"Active"},
                 {"productId":"9NDLGGH2JHXQ","productType":"Durable","status":"Active"}
               ]}"#,
        )
        .unwrap();
        let skipped: Vec<(String, &'static str)> = library::skipped(&entries)
            .into_iter()
            .map(|(e, why)| (e.product_id.clone(), why))
            .collect();
        let rows = library::join(&entries, &[]);
        let shown: Vec<&library::Row> = rows
            .iter()
            .filter(|r| r.entry.is_some_and(library::Entry::is_title))
            .collect();
        let doc = library_json(&shown, &skipped);
        assert_eq!(doc["schema"], JSON_SCHEMA);
        assert_eq!(doc["titles"].as_array().unwrap().len(), 1);
        assert_eq!(doc["skipped"][0]["product_id"], "9NDLGGH2JHXQ");
        assert!(doc["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not a game"));
    }

    #[test]
    fn doctor_lines_are_grouped_and_marked() {
        let checks = vec![
            Check::ok("host", "x86_64"),
            Check::fail("client", "no xodus-cli").with_hint("build it, or set XODUS_CLI_DIR"),
            Check::warn("client", "no Steam install").with_hint("set XODUS_STEAM_DIR"),
        ];
        let out = render_checks(&checks, &Style::plain());
        assert!(out.contains("== host =="), "{out}");
        assert!(out.contains("== client =="), "{out}");
        assert!(out.contains(" ok "), "{out}");
        assert!(out.contains(" !! "), "{out}");
        assert!(out.contains(" -- "), "{out}");
        assert!(out.contains("build it, or set XODUS_CLI_DIR"), "{out}");
        assert_eq!(failures(&checks), 1);
    }

    #[test]
    fn a_hint_on_a_passing_check_is_not_printed() {
        let checks = vec![Check::ok("host", "fine").with_hint("noise")];
        assert!(!render_checks(&checks, &Style::plain()).contains("noise"));
    }

    #[test]
    fn a_doctor_check_serialises_for_a_gui() {
        let v =
            serde_json::to_value(Check::warn("runtime", "publishes no capability list")).unwrap();
        assert_eq!(v["section"], "runtime");
        assert_eq!(v["status"], "warn");
        // Absent, not null: a GUI should not have to distinguish the two.
        assert!(v.get("hint").is_none());
    }

    #[test]
    fn the_32_bit_loader_check_names_the_real_loader_file() {
        assert!(has_i386_loader(Path::new("Cargo.toml")));
        assert!(!has_i386_loader(Path::new("/this/path/does/not/exist")));
    }

    #[test]
    fn colour_is_off_when_it_cannot_be_seen() {
        assert_eq!(Style::detect(false, false), Style::plain());
        assert_eq!(Style::detect(true, true), Style::plain());
    }

    #[test]
    fn a_dry_run_prints_something_a_shell_would_accept() {
        let plan = launch::Plan {
            program: PathBuf::from("/opt/xodus/xodus-cli"),
            args: vec!["run".into(), "-e".into(), "Minecraft.Windows.exe".into()],
            env_set: [("WINE_GAMEINPUT".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            env_unset: vec!["LD_PRELOAD".into()],
            cwd: None,
        };
        let out = render_plan(&plan);
        assert!(out.starts_with("unset LD_PRELOAD\n"), "{out}");
        assert!(out.contains("export WINE_GAMEINPUT=1\n"), "{out}");
        assert!(
            out.trim_end()
                .ends_with("/opt/xodus/xodus-cli run -e Minecraft.Windows.exe"),
            "{out}"
        );
    }

    #[test]
    fn a_path_with_a_space_survives_being_quoted() {
        assert_eq!(shell_quote("plain-value_1.2/x"), "plain-value_1.2/x");
        assert_eq!(
            shell_quote("/games/Forza Horizon 5"),
            "'/games/Forza Horizon 5'"
        );
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        // A value that would otherwise start a second command.
        assert_eq!(shell_quote("a; rm -rf /"), "'a; rm -rf /'");
    }

    #[test]
    fn indenting_leaves_blank_lines_blank() {
        assert_eq!(indent("a\n\nb\n", "  "), "  a\n\n  b\n");
    }

    // -- exit codes ---------------------------------------------------------

    #[test]
    fn a_childs_exit_status_becomes_ours() {
        assert_eq!(exit_code_from(Some(0), None), 0);
        assert_eq!(exit_code_from(Some(1), None), 1);
        // Every GDK title exits 1 without the inherited-fd patch, so a distinct
        // code has to survive intact rather than being flattened to "failure".
        assert_eq!(exit_code_from(Some(3), None), 3);
        assert_eq!(exit_code_from(Some(255), None), 255);
    }

    #[test]
    fn a_signalled_child_reports_128_plus_the_signal_like_a_shell() {
        assert_eq!(exit_code_from(None, Some(9)), 137);
        assert_eq!(exit_code_from(None, Some(15)), 143);
    }

    #[test]
    fn a_status_that_makes_no_sense_is_a_plain_failure() {
        assert_eq!(exit_code_from(None, None), EXIT_FAILURE);
        assert_eq!(exit_code_from(Some(-1), None), EXIT_FAILURE);
        assert_eq!(exit_code_from(None, Some(200)), EXIT_FAILURE);
    }

    // -- recipes written from a package ------------------------------------

    fn package(name: &str, files: &[&str]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferestre-detected-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        for file in files {
            std::fs::write(dir.join(file), b"").expect("write");
        }
        dir
    }

    fn detected(dir: &Path, executable: &str) -> Recipe {
        let mut recipe = Recipe::blank("9ZZDETECTED1", "Detected");
        recipe.launch.executable = executable.into();
        detected_requirements(&mut recipe, dir);
        recipe
    }

    #[test]
    fn a_package_without_a_game_config_only_requires_the_main_image() {
        let dir = package("plain", &["Game.exe"]);
        let recipe = detected(&dir, "Game.exe");
        assert_eq!(recipe.runtime.requires, [capability::MEMFD_MAIN_IMAGE]);
        assert!(recipe.runtime.wants.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_gdk_package_requires_what_a_probe_can_see_and_wants_the_rest() {
        // Any case of the config name is the same file to the package.
        let dir = package("gdk", &["Game.exe", "microsoftgame.CONFIG"]);
        let recipe = detected(&dir, "Game.exe");
        assert_eq!(
            recipe.runtime.requires,
            [capability::MEMFD_MAIN_IMAGE, capability::XGAMERUNTIME_USER]
        );
        assert_eq!(recipe.runtime.wants, [capability::PACKAGE_IDENTITY]);

        // The point of the split: a runtime that publishes nothing is still
        // selected, and the name the probe cannot see becomes a warning.
        let probed = InstalledRuntime {
            path: PathBuf::from("/opt/probed"),
            version: None,
            provides: names(&[
                capability::MEMFD_MAIN_IMAGE,
                capability::XGAMERUNTIME_GAMESAVE,
                capability::XGAMERUNTIME_NETWORKING,
                capability::XGAMERUNTIME_PACKAGE,
                capability::XGAMERUNTIME_TASKQUEUE,
                capability::XGAMERUNTIME_USER,
            ]),
            source: CapabilitySource::Probed,
        };
        let runtimes = [probed];
        let selected = runtime::select(&recipe, &runtimes, None, None).expect("selected");
        assert_eq!(
            gate(&runtime::assess(&recipe, selected, None)),
            Gate::Degraded
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_package_with_a_known_engine_records_its_want() {
        let dir = package(
            "engine",
            &[
                "RetroClassics.exe",
                "MicrosoftGame.Config",
                "Ultralight.dll",
                "UltralightCore.dll",
                "WebCore.dll",
            ],
        );
        let recipe = detected(&dir, "RetroClassics.exe");
        assert_eq!(
            recipe.runtime.wants,
            [
                capability::PACKAGE_IDENTITY,
                capability::RECORDING_ALLOCATOR_LIFETIME
            ]
        );
        // A default inferred at launch is not written down as configuration.
        assert!(recipe.launch.env.is_empty());
        let text = recipe.to_toml().expect("serialises");
        Recipe::parse(&text).expect("and reads back");
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- the repository's own data -----------------------------------------

    #[test]
    fn the_repositorys_own_recipes_render() {
        // The real recipes are the contract; this catches a schema change that
        // breaks the listing before someone runs `ferestre titles` and sees it.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !dir.exists() {
            return; // packaged crate without the repository around it
        }
        let recipes = Recipe::load_dir(&dir).expect("repository recipes should parse");
        let out = table(&titles_table(&recipes));
        assert!(out.contains("PRODUCT ID"), "{out}");
        for r in &recipes {
            assert!(
                out.contains(&r.title.product_id),
                "{} missing from:\n{out}",
                r.title.product_id
            );
        }
        let rows: Vec<TitleRow> = recipes.iter().map(TitleRow::from).collect();
        serde_json::to_value(&rows).expect("every recipe serialises");
    }

    #[test]
    fn every_setup_action_in_the_repository_is_one_this_build_implements() {
        // titles/SCHEMA.md: adding an action means implementing it here. This
        // is the half of that promise a recipe author cannot check by reading.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !dir.exists() {
            return;
        }
        for recipe in Recipe::load_dir(&dir).expect("recipes parse") {
            for step in &recipe.setup {
                assert_eq!(
                    step.action, "substitute-xcurl",
                    "{} asks for setup action '{}', which run_setup does not implement",
                    recipe.title.name, step.action
                );
            }
        }
    }
}
