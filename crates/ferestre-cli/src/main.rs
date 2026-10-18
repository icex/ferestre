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
        /// A library folder to put it in: the title gets its own folder inside
        /// it, named as in the games directory. The title's own folder is used
        /// as it is, to repair or update it. Defaults to the recipe's install
        /// directory.
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
    let results = check_updates(&paths, &xodus_cli)?;
    if cli.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "updates": results }));
    } else if results.is_empty() {
        println!("no installed packages could be checked");
    } else {
        for result in results {
            println!(
                "{}: {}",
                result["product_id"].as_str().unwrap_or("package"),
                update_state(&result)
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Ask the update service about every recorded install.
///
/// One title the service could not answer for is reported and skipped; it
/// used to end the whole check, and the titles after it were never asked.
fn check_updates(paths: &Paths, xodus_cli: &Path) -> Result<Vec<Value>> {
    let records: Vec<_> = install::all(paths.state_dir())
        .into_iter()
        .filter(|record| !record.is_stale())
        .collect();
    let roles = accounts::load(paths.config_dir())?;
    let mut results = Vec::new();
    for record in records {
        let Some(content_id) = record.content_ids.first() else {
            continue;
        };
        let mut command = update_command(xodus_cli, content_id);
        if let Some(roles) = &roles {
            roles.apply(&mut command);
        }
        let output = match command.output() {
            Ok(output) => output,
            Err(error) => {
                eprintln!(
                    "-- {}: could not run {}: {error}",
                    record.product_id,
                    xodus_cli.display()
                );
                continue;
            }
        };
        if !output.status.success() {
            eprintln!("-- {}: update service unavailable", record.product_id);
            continue;
        }
        let value: Value = match serde_json::from_slice(&output.stdout) {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "-- {}: the update client's answer was not JSON ({error})",
                    record.product_id
                );
                continue;
            }
        };
        let version = value["version"].as_str().unwrap_or_default().to_string();
        let available = record.update_available(Some(&version));
        results.push(json!({
            "product_id": record.product_id,
            "installed_version": record.package_version,
            "available_version": version,
            "update_available": available,
        }));
    }
    Ok(results)
}

/// The text for one title's answer. `null` is "cannot tell" -- a version is
/// missing on one side -- and is said so rather than shown as up to date.
fn update_state(result: &Value) -> &'static str {
    match result["update_available"] {
        Value::Bool(true) => "update available",
        Value::Bool(false) => "up to date",
        _ => "unknown (no version to compare)",
    }
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
    let request = LaunchRequest {
        dry_run,
        force,
        steam,
    };
    let prepared = prepare_launch(cli, &paths, title, request)?;
    if dry_run {
        // Nothing has been started or written: a dry run prints and stops.
        if cli.json {
            print_json(&json!({
                "schema": JSON_SCHEMA,
                "plan": plan_json(&prepared.plan, cli.show_account),
            }));
        } else {
            print!("{}", render_plan(&prepared.plan, cli.show_account));
        }
        return Ok(ExitCode::SUCCESS);
    }
    launch_prepared(&paths, prepared, force)
}

/// What `run` was asked for, beyond the title.
#[derive(Debug, Clone, Copy, Default)]
struct LaunchRequest {
    dry_run: bool,
    force: bool,
    steam: bool,
}

/// A launch worked out and checked, with nothing started yet.
struct PreparedLaunch {
    recipe: Recipe,
    runtime: InstalledRuntime,
    registry: Option<Registry>,
    dirs: launch::Dirs,
    options: launch::Options,
    plan: launch::Plan,
    service_socket: Option<PathBuf>,
}

/// Everything a launch decides before it starts anything: the recipe, the
/// runtime and whether it may run the title, the account roles, and the plan.
///
/// Reads only, apart from describing an undescribed title, which a dry run
/// does in memory: `--dry-run` must not leave anything behind.
fn prepare_launch(
    cli: &Cli,
    paths: &Paths,
    title: &str,
    request: LaunchRequest,
) -> Result<PreparedLaunch> {
    let (dir, mut recipes) = recipes_from(cli, paths)?;

    // Nothing describes it, but it is on disk and its package says how to start
    // it -- so describe it and carry on rather than stopping to ask. The
    // manifest is the authority here: it names the entry point, which is the
    // one thing a person would otherwise have to go and find in a tree of
    // thousands of files. Writing it down means the answer survives, and stays
    // editable when a title turns out to need something else.
    if !recipes.iter().any(|r| r.matches(title)) {
        if let Some((detected, written)) = describe_from_disk(paths, title, !request.dry_run)? {
            match written {
                Some(written) => eprintln!(
                    "-- no recipe, so one was written from the package manifest: {}",
                    written.display()
                ),
                None => {
                    eprintln!("-- no recipe; a launch would write one from the package manifest")
                }
            }
            recipes.push(detected);
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
    let runtimes = runtime::all(paths);
    // The title's own choice, else XODUS_PROTON_DIR, which is a choice made
    // for every title at once.
    let requested_runtime = runtime::requested(paths, &recipe.title.product_id)?;
    let registry = registry_for(paths);
    let runtime = choose_runtime(
        &inferred.recipe,
        &runtimes,
        requested_runtime.as_deref(),
        registry.as_ref(),
    )?;
    let recipe = engine_defaults(inferred, runtime, paths);
    let assessment = runtime::assess(&recipe, runtime, registry.as_ref());
    let verdict = gate(&assessment);
    if verdict == Gate::Refused && !request.force {
        bail!("{}", refusal(&assessment, runtime));
    }
    for line in warnings(&recipe, &assessment, verdict, request.force) {
        eprintln!("{line}");
    }

    let dirs = launch_dirs_for_runtime(paths, &runtime.path)?;
    let roles = accounts::launch_selection(paths.config_dir())?;
    let mut prefix = paths.prefix_dir(&recipe)?;
    // `XGR_XUID` is the Windows install's XUID, so it names the imported
    // player's save folders. Another player has their own prefix and their own
    // XUID: inheriting it would name their saves after somebody else, and
    // report that XUID whenever the live lookup has not answered yet.
    let mut own_xuid = true;
    if let Some(roles) = &roles {
        // Read even for a dry run: the Player decides which prefix the plan
        // names.
        let profiles = accounts::fetch(&dirs.xodus_cli())?;
        let (store, player) = roles.selected(&profiles)?;
        if roles.split() && !runtime.provides(accounts::CAPABILITY) {
            bail!("This runtime cannot separate Store and Player identities. Select an account-sharing runtime, or clear the account role selections.");
        }
        if !runtime.provides(SERVICE_SOCKET_OVERRIDE) {
            bail!("This runtime cannot isolate the selected account service. Update the runtime first.");
        }
        if !request.dry_run {
            accounts::require_service(&dirs.xodus_service())?;
        }
        prefix = accounts::player_prefix(&prefix, player)?;
        own_xuid = player.legacy_saves;
        if cli.show_account {
            eprintln!(":: Store: {}; Player: {}", store.label, player.label);
        } else {
            eprintln!(":: using the selected Store and Player accounts");
        }
    }
    let options = launch::Options {
        steam_integration: request.steam,
        xuid: paths
            .var("XGR_XUID")
            .filter(|_| own_xuid)
            .map(str::to_string),
        game_dir: Some(paths.install_dir(&recipe)?),
        prefix_dir: Some(prefix),
    };
    let mut plan = launch::plan(&recipe, &dirs, &options);
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
        bind_service_title(
            &mut plan,
            &recipe.title.product_id,
            content,
            &launch::game_dir(&recipe, &dirs, &options),
        )?;
    }
    let service_socket = plan_service_socket(&mut plan, &dirs, runtime, roles.as_ref())?;
    Ok(PreparedLaunch {
        runtime: runtime.clone(),
        recipe,
        registry,
        dirs,
        options,
        plan,
        service_socket,
    })
}

/// Start a prepared launch and wait for the title.
fn launch_prepared(paths: &Paths, prepared: PreparedLaunch, force: bool) -> Result<ExitCode> {
    let PreparedLaunch {
        recipe,
        runtime,
        registry,
        dirs,
        options,
        plan,
        service_socket,
    } = prepared;
    // Held while the title runs: an install, update or removal of it waits
    // for this, and this does not start while one of those is under way.
    let _lock = TitleLock::take(paths, &recipe.title.product_id, LockMode::Run)?;
    // Through the checked path: the slug is recipe data, and the log is
    // created wherever this says.
    let log = paths.log_file(&recipe)?;

    // The three things a Plan deliberately does not model, in the order
    // launch-gdk.sh does them.
    let game_dir = launch::game_dir(&recipe, &dirs, &options);
    if !game_dir.is_dir() {
        bail!(
            "{} is not installed at {} -- download it: ferestre install {}",
            recipe.title.name,
            game_dir.display(),
            recipe.title.product_id
        );
    }
    // A download that stopped partway leaves the client's working container;
    // the tree beside it is part old build, part new, and part missing.
    if install::is_partial(&game_dir) {
        if !force {
            bail!(
                "{} did not finish downloading at {} -- finish it: ferestre install {} \
                 (or launch what is there anyway with --force)",
                recipe.title.name,
                game_dir.display(),
                recipe.title.product_id
            );
        }
        eprintln!(
            "-- --force: launching an unfinished download in {}",
            game_dir.display()
        );
    }
    let prefix = launch::prefix_dir(&recipe, &dirs, &options);
    if let Some(registry) = &registry {
        register_winrt_classes(&prefix, registry, &runtime);
    }
    run_setup(&recipe, "every-launch", &dirs.scripts_dir, &game_dir)?;
    std::fs::create_dir_all(&prefix).context("creating the Wine prefix directory")?;
    if runtime.provides("xgameruntime.playfab-file-saves") {
        if let Some(identity) = install::package_identity(&game_dir) {
            let backups = paths
                .state_dir()
                .join("save-backups")
                .join(&recipe.title.product_id);
            for backup in ferestre_core::playfab::repair_package(&prefix, &identity, &backups)? {
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
    forwarding.set_session(Some(Arc::new(PrefixSession::for_runtime(
        &runtime.path,
        &prefix,
        &recipe.launch.executable,
    ))));
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
    run_teed(plan.to_command(), &log, &forwarding)
}

/// The runtime a launch uses, whether or not it can run the title.
///
/// Deciding that is the gate's job, not this one's: a runtime filtered out here
/// left only "no installed runtime satisfies X", with no list of what was
/// missing, `--force` doing nothing, and a probed runtime -- which cannot be
/// checked, only warned about -- refused outright. So: the runtime chosen for
/// the title if it is still installed; otherwise the newest that satisfies the
/// recipe; otherwise the newest there is, for the gate to judge.
fn choose_runtime<'a>(
    recipe: &Recipe,
    runtimes: &'a [InstalledRuntime],
    requested: Option<&Path>,
    registry: Option<&Registry>,
) -> Result<&'a InstalledRuntime> {
    runtime::choose(recipe, runtimes, requested, registry).map(|choice| choice.runtime)
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
    let paths = Paths::from_env()?;
    install_title(cli, &paths, product_id, dir, &cancellation)
}

fn install_title(
    cli: &Cli,
    paths: &Paths,
    product_id: &str,
    dir: Option<&Path>,
    cancellation: &cancel::Cancellation,
) -> Result<ExitCode> {
    let product_id = normalise_product_id(product_id);
    let xodus_cli = paths
        .xodus_cli()
        .ok_or_else(|| anyhow!("no xodus-cli found; set XODUS_CLI_DIR (see: ferestre doctor)"))?;
    // Not swallowed: with a recipe that does not parse, this would download to
    // the wrong place, skip the title's setup, and then write a blank recipe
    // over the person's own.
    let recipes = recipes_for_change(cli, paths)?;
    let recipe = recipes.iter().find(|r| r.matches(&product_id));

    // A recipe knows the layout a title expects -- Bedrock's package lives one
    // level down -- so it decides the destination when there is one.
    let dest = install_destination(paths, &product_id, recipe, dir)?;

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

    // Held until the command ends: a launch must not start on a tree that is
    // being rewritten, and a running title's files must not be replaced.
    let _lock = TitleLock::take(paths, &product_id, LockMode::Change)?;

    // Remembered so a failed install can put the disk back the way it found it.
    let dest_existed = dest.is_dir();
    let before = container_state(&dest);
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
    // An update the client gave up on still exits zero -- a lapsed licence, too
    // little disk space -- and leaves the previous build's manifest and
    // container where they were, so the checks above pass. Two things tell it
    // apart from a finished run: the client's working container is renamed
    // over the old one only once every file is written, so one still there
    // means it stopped early; and a finished run always replaces the
    // container, so the very same file as before means nothing was installed.
    if let Some(problem) = unfinished_update(&dest, before.as_ref()) {
        if before.is_some() {
            eprintln!("!! {product_id} was not updated: {problem}");
            eprintln!(
                "   the reason is in the client's output above; the build that was installed \
                 in {} is still there, and `ferestre install {product_id}` tries again",
                dest.display()
            );
        } else {
            eprintln!("!! {product_id} did not finish installing: {problem}");
            eprintln!(
                "   the reason is in the client's output above; the files are kept in {}, \
                 and `ferestre install {product_id}` carries on from them",
                dest.display()
            );
        }
        return Ok(ExitCode::FAILURE);
    }

    // Recorded before the setup steps run: the files are there and the
    // version can be read, whatever a setup step then does. A setup step that
    // fails used to leave a finished download unrecorded, which the window
    // then had no way to remove.
    cancellation.check()?;
    match record_install(paths, &product_id, &dest) {
        Ok(record) => eprintln!("{}", installed_summary(&record)),
        // The title is installed either way. Failing the command over the
        // bookkeeping would be the tail wagging the dog.
        Err(e) => eprintln!("-- installed, but could not record the version: {e}"),
    }

    // A download replaces whatever was patched last time -- Microsoft's
    // XCurl.dll comes back with every update -- so the after-install steps run
    // again here, not once at first install. They need the scripts and nothing
    // else: asking for a runtime here failed every install on a machine whose
    // only runtime is one `install-runtime` downloaded.
    let mut setup_failed = false;
    if let Some(recipe) = recipe {
        let scripts = paths
            .scripts_dir()
            .ok_or_else(|| anyhow!("cannot find the scripts directory; set XODUS_REPO_DIR"));
        if let Err(error) = scripts.and_then(|s| run_setup(recipe, "after-install", s, &dest)) {
            print_error(&error);
            eprintln!(
                "   {} is installed and recorded, but its setup did not finish; \
                 `ferestre install {product_id}` runs it again",
                recipe.title.name
            );
            setup_failed = true;
        }
    }

    // A title nobody has described can describe itself now that it is on disk.
    // This is the moment it becomes possible: the package manifest names the
    // executable, and before the download there was no manifest to read. Doing
    // it here rather than in the window means `ferestre install <anything>`
    // leaves something runnable whichever way it was invoked.
    if recipe.is_none() {
        match write_detected_recipe(paths, &product_id, &dest) {
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
    Ok(if setup_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Why an update that exited zero did not install anything, if it did not.
fn unfinished_update(dest: &Path, before: Option<&ContainerState>) -> Option<&'static str> {
    if install::is_partial(dest) {
        return Some("the client stopped before every file was written");
    }
    if before.is_some() && container_state(dest).as_ref() == before {
        return Some("the client did not replace the package, so nothing changed");
    }
    None
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
    let paths = Paths::from_env()?;
    let mut stdin = std::io::stdin().lock();
    uninstall_title(
        cli,
        &paths,
        product_id,
        Removal {
            keep_files,
            remove_prefix,
            yes,
        },
        &mut stdin,
    )
}

/// What `uninstall` was asked to do.
#[derive(Debug, Clone, Copy)]
struct Removal {
    keep_files: bool,
    remove_prefix: bool,
    yes: bool,
}

fn uninstall_title(
    cli: &Cli,
    paths: &Paths,
    title: &str,
    removal: Removal,
    answers: &mut dyn BufRead,
) -> Result<ExitCode> {
    let recipes = recipes_for_change(cli, paths)?;
    // Only a product id or a recipe's slug. Anything else used to fall
    // through to `<games dir>/<what was typed>`: an empty id named the games
    // directory itself, and `..` named the home directory.
    let product_id = if is_product_id(title) {
        normalise_product_id(title)
    } else if let Some(recipe) = recipes
        .iter()
        .find(|r| r.title.slug.eq_ignore_ascii_case(title))
    {
        recipe.title.product_id.to_ascii_uppercase()
    } else {
        return Err(usage(&format!(
            "'{title}' is not a Store product id (12 characters, letters and digits) or the \
             slug of a recipe (see: ferestre titles)"
        )));
    };
    let recipe = recipes.iter().find(|r| r.matches(&product_id));

    // The record first, then the recipe: the record is what the launcher acted
    // on, and a recipe's install directory is where it *would* have gone.
    let record = install::load(paths.state_dir(), &product_id);
    let dir = match &record {
        Some(record) => Some(record.dir.clone()),
        None => recipe
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
    let removing = !removal.keep_files && dir.is_dir();

    // The Wine prefix is a separate directory and it outlives the install: it
    // is where a title's saved games are, so removing it silently alongside the
    // files would throw away the thing somebody most wants to keep. Named
    // either way, because a directory left behind that nothing mentions is how
    // `stardew-valley-proton` sat there after Stardew Valley was uninstalled.
    // Every Player's own copy is named too: with account roles, a Player's
    // saves are in `players/<namespace>/<prefix>`, not in the shared one.
    let prefixes = recipe
        .and_then(|r| paths.prefix_dir(r).ok())
        .map(|base| title_prefixes(&base))
        .unwrap_or_default();

    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "product_id": product_id,
            "directory": dir.display().to_string(),
            "removing_files": removing,
            "had_record": record.is_some(),
            "prefix": prefixes.first().map(|p| p.display().to_string()),
            "prefixes": prefixes.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "removing_prefix": removal.remove_prefix && !prefixes.is_empty(),
        }));
        return Ok(ExitCode::SUCCESS);
    }

    // Checked before anything is asked, so the question is never put for a
    // directory that would then be refused.
    if removing {
        check_removable(&dir, paths, &product_id, Removable::Install)?;
    }
    if removal.remove_prefix {
        for prefix in &prefixes {
            check_removable(prefix, paths, &product_id, Removable::Prefix)?;
        }
    }
    // Not while it runs, and not while something else is installing it.
    let _lock = TitleLock::take(paths, &product_id, LockMode::Change)?;

    // Everything that is about to go, named with its size, and asked about
    // once.
    let mut doomed: Vec<&Path> = Vec::new();
    if removing {
        doomed.push(&dir);
    }
    if removal.remove_prefix {
        doomed.extend(prefixes.iter().map(PathBuf::as_path));
    }
    for (i, path) in doomed.iter().enumerate() {
        eprintln!(
            ":: removing {}{} ({})",
            if i > 0 || !removing {
                "the Wine prefix "
            } else {
                ""
            },
            path.display(),
            ferestre_core::human_bytes(install::tree_size(path))
        );
    }
    if !doomed.is_empty() && !removal.yes {
        // Asked, not assumed. This is the only command here that destroys
        // something, and `--yes` exists for the window, which asks first in
        // its own dialog.
        eprint!(
            "   delete {}? [y/N] ",
            if doomed.len() == 1 { "it" } else { "them" }
        );
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        answers.read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            eprintln!("-- left alone");
            return Ok(ExitCode::FAILURE);
        }
    }
    if removing {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        eprintln!("-- removed {}", dir.display());
    } else if !removal.keep_files {
        eprintln!(":: {} is already gone", dir.display());
    }

    for prefix in &prefixes {
        if removal.remove_prefix {
            std::fs::remove_dir_all(prefix)
                .with_context(|| format!("removing {}", prefix.display()))?;
            eprintln!("-- removed the Wine prefix {}", prefix.display());
        } else {
            eprintln!(
                "-- kept the Wine prefix {} ({}), which is where saved games live",
                prefix.display(),
                ferestre_core::human_bytes(install::tree_size(prefix))
            );
        }
    }
    if !removal.remove_prefix && !prefixes.is_empty() {
        eprintln!("   remove them too with: ferestre uninstall {product_id} --prefix");
    }

    install::forget(paths.state_dir(), &product_id)?;
    eprintln!("-- forgot the install record for {product_id}");
    if removal.keep_files {
        eprintln!("   the files are still in {}", dir.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// A title's Wine prefixes that are on disk: the shared one, then each
/// Player's own (`players/<namespace>/<prefix name>` beside it), sorted.
fn title_prefixes(base: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if base.is_dir() {
        found.push(base.to_path_buf());
    }
    let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
        return found;
    };
    let mut players: Vec<PathBuf> = std::fs::read_dir(parent.join("players"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|namespace| namespace.path().join(name))
        .filter(|prefix| prefix.is_dir() && !prefix.is_symlink())
        .collect();
    players.sort();
    found.extend(players);
    found
}

// ---------------------------------------------------------------------------
// install destinations, removal and per-title locks
// ---------------------------------------------------------------------------

/// The client keeps its container here once a download has finished. Until
/// then it writes [`install::PARTIAL_CONTAINER`] and renames it over this one
/// only once every file is written ([`install::is_partial`]).
const STREAMING_CONTAINER: &str = ".xodus-streaming.msixvc";

/// Where `install` puts a title.
///
/// A folder the person chose is a library to install *into*: the title goes
/// in its own folder inside it, named as it would be in the games directory.
/// Installing straight into the chosen folder mixed the package into
/// whatever was there, recorded the whole folder as the title, and a later
/// Remove deleted all of it. The exception is the title's own folder: the
/// one it is installed in, chosen again to repair or update it, or the one it
/// gets in the games directory, which is what a window that still offers the
/// title's folder rather than a library passes for a first install.
fn install_destination(
    paths: &Paths,
    product_id: &str,
    recipe: Option<&Recipe>,
    chosen: Option<&Path>,
) -> Result<PathBuf> {
    let dest = match (chosen, recipe) {
        (Some(chosen), _) => {
            let chosen = absolute_dir(paths, chosen)?;
            if is_default_folder(paths, product_id, recipe, &chosen)? {
                // What the window fills in: the title's own folder in the
                // games directory, not a library to put it in.
                chosen
            } else {
                let typed = chosen.to_str().ok_or_else(|| {
                    usage(&format!(
                        "{}: the install folder has to be valid UTF-8",
                        chosen.display()
                    ))
                })?;
                let folder = game_folder(recipe, product_id)?;
                let folder = folder.to_str().unwrap_or_default();
                // The library rules: the title gets its own folder inside, a
                // folder that is already this title's (its record, its
                // package, or its unfinished first download) is used as it
                // is, and another title's folder, somebody's own files and
                // the launcher's own directories are refused.
                install::resolve_destination(typed, folder, product_id, paths)
                    .map_err(|e| usage(&format!("{e:#}")))?
            }
        }
        (None, Some(recipe)) => paths.install_dir(recipe)?,
        // An update of a title with no recipe goes where it was installed,
        // not into a second copy under the games directory.
        (None, None) => install::load(paths.state_dir(), product_id)
            .map(|record| record.dir)
            .filter(|dir| dir.is_absolute() && dir.is_dir())
            .unwrap_or_else(|| paths.games_dir().join(product_id.to_ascii_lowercase())),
    };
    if let Some(reason) = protected_reason(&dest, paths) {
        return Err(usage(&format!(
            "{} is {reason}; choose a folder to install into",
            dest.display()
        )));
    }
    if let Some(other) = package_store_id(&dest).filter(|id| id != product_id) {
        return Err(usage(&format!(
            "{} already holds another title ({other}); choose another folder",
            dest.display()
        )));
    }
    Ok(dest)
}

/// The folder a title gets inside a library: the recipe's install directory
/// (Bedrock's is `bedrock/game`), or the product id, as in the games
/// directory.
fn game_folder(recipe: Option<&Recipe>, product_id: &str) -> Result<PathBuf> {
    let name = match recipe {
        Some(recipe) => recipe.install_dir().to_string(),
        None => product_id.to_ascii_lowercase(),
    };
    let folder = Path::new(&name);
    let normal = folder
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if name.is_empty() || !normal {
        bail!("{product_id}: install dir \"{name}\" must be a relative path with no \"..\"");
    }
    Ok(folder.to_path_buf())
}

/// A folder as typed, made absolute.
///
/// `~` is expanded: the window passes its "Install to" text through as typed,
/// and without a shell in between a literal `~` folder appeared under
/// whatever the working directory was. A relative folder is taken from the
/// working directory, as a shell user means it, and kept absolute, so the
/// record names the same place from the window and from Steam.
fn absolute_dir(paths: &Paths, typed: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let mut components = typed.components();
    let expanded = match components.clone().next() {
        Some(Component::Normal(first)) if first == "~" => {
            components.next();
            paths.home().join(components.as_path())
        }
        Some(Component::Normal(first)) if first.to_string_lossy().starts_with('~') => {
            return Err(usage(&format!(
                "{}: only ~ on its own is expanded; write the path out",
                typed.display()
            )));
        }
        Some(_) => typed.to_path_buf(),
        None => return Err(usage("the install folder is empty")),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .context("finding the working directory")?
            .join(expanded)
    };
    if !absolute
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Ok(absolute);
    }
    // `..` means something only once the folders it climbs out of exist.
    std::fs::canonicalize(&absolute).map_err(|_| {
        usage(&format!(
            "{}: a folder with \"..\" in it has to exist; write the path out",
            typed.display()
        ))
    })
}

/// Whether `dir` is the folder the title gets when no folder is chosen: the
/// one `ferestre install` with no folder would use, or its place in the
/// games directory.
fn is_default_folder(
    paths: &Paths,
    product_id: &str,
    recipe: Option<&Recipe>,
    dir: &Path,
) -> Result<bool> {
    let mut defaults = vec![paths.games_dir().join(game_folder(recipe, product_id)?)];
    if let Some(dir) = recipe.and_then(|recipe| paths.install_dir(recipe).ok()) {
        defaults.push(dir);
    }
    Ok(defaults
        .iter()
        .any(|default| default == dir || same_dir(default, dir)))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The Store id the package in `dir` names in its `MicrosoftGame.config`,
/// upper case.
fn package_store_id(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(install::game_config(dir)?).ok()?;
    let open = "<StoreId>";
    let start = text.find(open)? + open.len();
    let value = text[start..].split_once("</StoreId>")?.0.trim();
    (!value.is_empty()).then(|| value.to_ascii_uppercase())
}

/// Why `dir` is a directory no title owns, if it is one: the home directory,
/// the games directory, the launcher's own directories, or a folder holding
/// any of them.
fn protected_reason(dir: &Path, paths: &Paths) -> Option<String> {
    let resolved = resolve_existing(dir);
    let mut protected: Vec<(PathBuf, &str)> = vec![
        (paths.home().to_path_buf(), "the home directory"),
        (paths.games_dir().to_path_buf(), "the games directory"),
        (
            paths.games_dir().join("players"),
            "where Players' saves are kept",
        ),
        (
            paths.state_dir().to_path_buf(),
            "the launcher's state directory",
        ),
        (paths.config_dir().to_path_buf(), "the launcher's settings"),
        (paths.cache_dir().to_path_buf(), "the launcher's cache"),
        (paths.runtimes_dir(), "where runtimes are installed"),
    ];
    for (dir, what) in [
        (paths.titles_dir(), "the recipe directory"),
        (paths.scripts_dir(), "the launcher's scripts"),
        (paths.cli_dir(), "the Xbox client's directory"),
        (paths.tree_dir(), "the launcher's installation"),
        (paths.steam_dir(), "the Steam directory"),
    ] {
        if let Some(dir) = dir {
            protected.push((dir.to_path_buf(), what));
        }
    }
    for name in ["XDG_RUNTIME_DIR", "XODUS_RUNTIME_DIR"] {
        if let Some(dir) = paths
            .var(name)
            .map(PathBuf::from)
            .filter(|d| d.is_absolute())
        {
            protected.push((dir, "the session's runtime directory"));
        }
    }
    protected.into_iter().find_map(|(other, what)| {
        let other = resolve_existing(&other);
        if other == resolved {
            Some(what.to_string())
        } else if other.starts_with(&resolved) {
            Some(format!("a folder holding {what} ({})", other.display()))
        } else {
            None
        }
    })
}

/// A path with its symlinks and `..` resolved as far as it exists, so two
/// spellings of one directory compare equal.
fn resolve_existing(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(existing) {
            return rest
                .iter()
                .rev()
                .fold(resolved, |acc: PathBuf, part: &&std::ffi::OsStr| {
                    acc.join(part)
                });
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Which kind of directory is about to be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removable {
    Install,
    Prefix,
}

/// Refuse to delete anything that is not this title's own.
///
/// A record is a JSON file anybody can edit, a recipe can name a directory
/// through an environment variable, and an install directory used to be
/// whatever was typed, so the path is checked here and not trusted: it is
/// never the home, games or launcher directories or a folder holding them,
/// never holds another title's install, and an install directory never holds
/// Wine prefixes -- that is a games library, not a title.
fn check_removable(dir: &Path, paths: &Paths, product_id: &str, kind: Removable) -> Result<()> {
    install::safe_to_remove(dir)?;
    // The core's rules for the directories this command was given, which
    // `safe_to_remove` can only apply for the ones the environment names.
    install::check_removable(dir, paths)?;
    if dir
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("{} has \"..\" in it; remove it by hand", dir.display());
    }
    if let Some(reason) = protected_reason(dir, paths) {
        bail!("{} is {reason}; refusing to delete it", dir.display());
    }
    let resolved = resolve_existing(dir);
    for other in install::all(paths.state_dir()) {
        if other.product_id.eq_ignore_ascii_case(product_id) {
            continue;
        }
        if resolve_existing(&other.dir).starts_with(&resolved) {
            bail!(
                "{} holds {}'s install ({}); refusing to delete it",
                dir.display(),
                other.product_id,
                other.dir.display()
            );
        }
    }
    if kind == Removable::Install {
        if let Some(inside) = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .find(|child| holds_other_titles(child, product_id))
        {
            bail!(
                "{} looks like a games library, not one title's folder ({} is in it); \
                 refusing to delete it",
                dir.display(),
                inside.display()
            );
        }
    }
    Ok(())
}

/// Whether `child`, inside a folder about to be deleted, belongs to the games
/// library rather than to the title: a Wine prefix (`<slug>-proton` with its
/// `pfx`), the Players' prefixes, or another title's download. Told by what
/// is inside, not the name alone, so a game's own `players` folder is not
/// mistaken for one.
fn holds_other_titles(child: &Path, product_id: &str) -> bool {
    if !child.is_dir() || child.is_symlink() {
        return false;
    }
    let name = child.file_name().unwrap_or_default().to_string_lossy();
    if name.ends_with("-proton") && child.join("pfx").is_dir() {
        return true;
    }
    if name == "players"
        && std::fs::read_dir(child)
            .into_iter()
            .flatten()
            .flatten()
            .any(|namespace| {
                let name = namespace.file_name();
                let name = name.to_string_lossy();
                name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())
            })
    {
        return true;
    }
    child.join(STREAMING_CONTAINER).is_file()
        && package_store_id(child).is_some_and(|id| id != product_id)
}

/// What a per-title lock is held for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockMode {
    /// A launch, held while the title runs. Any number at once.
    Run,
    /// Install, update or remove. Only with nothing else holding the title.
    Change,
}

/// One title's claim, shared between launches and exclusive for anything that
/// rewrites or deletes its files.
///
/// Without it the window offered Remove for a title Steam had started, an
/// update rewrote a running game's files, and a launch could start halfway
/// through an update. Not waited for: the person is told what holds the title
/// instead of watching a command hang. Only an instant's grace, because a
/// process starting a child at the moment a lock is let go can hold a copy of
/// its descriptor until that child has started.
struct TitleLock {
    _file: std::fs::File,
}

impl TitleLock {
    fn take(paths: &Paths, product_id: &str, mode: LockMode) -> Result<Option<Self>> {
        use std::os::fd::AsRawFd;
        let directory = paths.state_dir().join("locks");
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("creating {}", directory.display()))?;
        let path = directory.join(format!("{}.lock", product_id.to_ascii_uppercase()));
        let file = open_lock(&path)?;
        let operation = match mode {
            LockMode::Run => libc::LOCK_SH,
            LockMode::Change => libc::LOCK_EX,
        } | libc::LOCK_NB;
        let patience = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            // Safety: a plain flock on a descriptor owned here.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(Some(Self { _file: file }));
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EWOULDBLOCK) if std::time::Instant::now() < patience => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Some(libc::EWOULDBLOCK) => bail!(match mode {
                    LockMode::Run => format!(
                        "{product_id} is being installed, updated or removed; \
                         wait for that to finish"
                    ),
                    LockMode::Change => format!(
                        "{product_id} is running, or another install or removal of it is \
                         under way; close it and try again"
                    ),
                }),
                Some(libc::ENOLCK | libc::EOPNOTSUPP | libc::ENOSYS) => {
                    eprintln!(
                        "-- {} cannot be locked ({error}); nothing stops a launch and an \
                         update of this title overlapping",
                        path.display()
                    );
                    return Ok(None);
                }
                _ => return Err(error).with_context(|| format!("locking {}", path.display())),
            }
        }
    }
}

/// Which file the package container is, so a run that replaced it can be told
/// from one that left it alone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContainerState {
    device: u64,
    inode: u64,
    modified: (i64, i64),
    length: u64,
}

fn container_state(dir: &Path) -> Option<ContainerState> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(dir.join(STREAMING_CONTAINER)).ok()?;
    Some(ContainerState {
        device: metadata.dev(),
        inode: metadata.ino(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        length: metadata.len(),
    })
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
/// the manifest names no entry point. With `write` false the description is
/// only returned, for a dry run, which must not leave a file behind.
fn describe_from_disk(
    paths: &Paths,
    title: &str,
    write: bool,
) -> Result<Option<(Recipe, Option<PathBuf>)>> {
    let product_id = normalise_product_id(title);
    let Some(record) = install::load(paths.state_dir(), &product_id) else {
        return Ok(None);
    };
    if !install::looks_installed(&record.dir) {
        return Ok(None);
    }
    let Some(recipe) = detected_recipe(paths, &product_id, &record.dir) else {
        return Ok(None);
    };
    let written = if write {
        Some(save_detected_recipe(paths, &recipe)?)
    } else {
        None
    };
    Ok(Some((recipe, written)))
}

/// Write a recipe for a title that had none, from what the package says.
///
/// `Ok(None)` when the manifest names no executable -- honest rather than a
/// guess, because a recipe pointing at the wrong binary produces a launch
/// failure that looks like the launcher's fault rather than an unfinished
/// description.
fn write_detected_recipe(paths: &Paths, product_id: &str, dest: &Path) -> Result<Option<PathBuf>> {
    match detected_recipe(paths, product_id, dest) {
        Some(recipe) => save_detected_recipe(paths, &recipe).map(Some),
        None => Ok(None),
    }
}

/// Save a detected recipe, never over a file that is already there: that file
/// is somebody's -- an edit, or a recipe this build could not read -- and a
/// guess must not replace it.
fn save_detected_recipe(paths: &Paths, recipe: &Recipe) -> Result<PathBuf> {
    let dir = paths.user_titles_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Only if absent, in one step: no gap between a check and the write.
    match recipe.save_new_to(&dir)? {
        Some(path) => Ok(path),
        None => bail!(
            "{} already exists and was left as it is",
            dir.join(format!(
                "{}.toml",
                recipe.title.product_id.to_ascii_uppercase()
            ))
            .display()
        ),
    }
}

fn detected_recipe(paths: &Paths, product_id: &str, dest: &Path) -> Option<Recipe> {
    let executable = install::executable(dest)?;

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
    // Written by the launcher, not reviewed: a recipe shipped for the title
    // later outranks it, and an edit in the window clears the mark.
    recipe.generated = true;
    detected_requirements(&mut recipe, dest);
    Some(recipe)
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
const RELEASES_API: &str = "https://api.github.com/repos/icex/ferestre/releases";

/// How many pages of the release feed are read looking for a runtime. Launcher
/// releases are frequent and runtime releases rare, so the newest runtime can
/// be well past the first page.
const RELEASE_PAGES: u32 = 10;
const RELEASES_PER_PAGE: usize = 100;

fn cmd_install_runtime(cli: &Cli) -> Result<ExitCode> {
    let cancellation = cancel::Cancellation::from_gui();
    let paths = Paths::from_env()?;
    install_runtime(
        cli,
        &paths,
        RELEASES_API,
        ferestre_core::signing::RELEASE_PUBLIC_KEY,
        &cancellation,
    )
}

fn install_runtime(
    cli: &Cli,
    paths: &Paths,
    releases_api: &str,
    public_key: &str,
    cancellation: &cancel::Cancellation,
) -> Result<ExitCode> {
    eprintln!("-- checking for the latest Ferestre runtime");
    let release = newest_runtime_release(|page| {
        http::get_text(&format!(
            "{releases_api}?per_page={RELEASES_PER_PAGE}&page={page}"
        ))
    })?;
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
    let installed = InstalledRuntime::at(&destination).is_ok();
    // The document and nothing else on stdout, whatever the answer.
    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "release": release.tag,
            "asset": release.asset_name,
            "destination": destination,
            "installed": installed,
        }));
        return Ok(ExitCode::SUCCESS);
    }
    if installed {
        eprintln!(
            "-- runtime {version} is already installed at {}",
            destination.display()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let cache = paths.cache_dir().join("runtime");
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;
    std::fs::create_dir_all(paths.runtimes_dir())?;
    // One at a time: two downloads of one archive wrote over each other's
    // partial file, and the cache then held a corrupt archive.
    let _lock = runtime_install_lock(&cache)?;
    remove_abandoned_installs(&paths.runtimes_dir());

    // Without a signature there is nothing to check a download against, and
    // no point making one.
    let signature_url = release.signature_url.as_deref().ok_or_else(|| {
        anyhow!(
            "runtime release {} publishes no signature ({}.minisig), so it cannot be \
             verified and was not installed",
            release.tag,
            release.asset_name
        )
    })?;
    let archive = cache.join(&release.asset_name);
    eprintln!("-- downloading runtime {version} (this is a one-time download)");
    // Checked against the release key before it is renamed into place; a
    // cached archive is checked like a fresh one and fetched again when it
    // does not verify, rather than reused by every later attempt.
    let downloaded = http::download_signed(
        &release.url,
        signature_url,
        &archive,
        || cancellation.requested(),
        public_key,
    );
    cancellation.check()?;
    downloaded.with_context(|| {
        format!(
            "could not download runtime {version} and verify it against the Ferestre release \
             key; nothing was installed, and no unverified file was kept"
        )
    })?;
    eprintln!("-- verified the signature of {}", release.asset_name);

    eprintln!("-- unpacking runtime {version}");
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
        // It verified, so it is the published archive; the next attempt
        // still starts from a fresh download rather than this file.
        let _ = std::fs::remove_file(&archive);
        bail!(
            "could not unpack {}; tar exited {}",
            archive.display(),
            code?
        );
    }
    let unpacked = temporary.join(format!("ferestre-runtime-{version}"));
    if let Err(error) = InstalledRuntime::at(&unpacked) {
        let _ = std::fs::remove_dir_all(&temporary);
        let _ = std::fs::remove_file(&archive);
        return Err(error);
    }
    if destination.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
        bail!(
            "{} appeared while the runtime was downloading; try again",
            destination.display()
        );
    }
    std::fs::rename(&unpacked, &destination)?;
    let _ = std::fs::remove_dir_all(&temporary);
    // Installed, so the archive is never read again: every runtime release
    // used to leave its full archive in the cache for good.
    let _ = std::fs::remove_file(&archive);
    eprintln!(
        "-- installed runtime {version} at {}",
        destination.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// The newest runtime release, looking past the first page of the feed.
///
/// The feed holds launcher and runtime releases together, newest first. Only
/// the first page was read, so once enough launcher releases followed the last
/// runtime, a new machine could find no runtime at all.
fn newest_runtime_release(fetch: impl Fn(u32) -> Result<String>) -> Result<runtime::Release> {
    for page in 1..=RELEASE_PAGES {
        let text = fetch(page)?;
        let releases: Vec<Value> =
            serde_json::from_str(&text).context("the release feed is not a JSON list")?;
        if let Ok(release) = runtime::latest_release(&text) {
            return Ok(release);
        }
        if releases.len() < RELEASES_PER_PAGE {
            break;
        }
    }
    bail!("no runtime archive was published in the Ferestre releases")
}

/// The lock that keeps two runtime installs apart. Not waited on: the person
/// is told one is already running.
fn runtime_install_lock(cache: &Path) -> Result<Option<std::fs::File>> {
    use std::os::fd::AsRawFd;
    let path = cache.join(".install.lock");
    let file = open_lock(&path)?;
    loop {
        // Safety: a plain flock on a descriptor owned here.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(file));
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EWOULDBLOCK) => {
                bail!("another runtime install is already running; wait for it to finish")
            }
            Some(libc::ENOLCK | libc::EOPNOTSUPP | libc::ENOSYS) => return Ok(None),
            _ => return Err(error).with_context(|| format!("locking {}", path.display())),
        }
    }
}

/// Unpacked trees an install left behind when it was killed. Each is named
/// for the process that made it, so a later install never removed another's.
fn remove_abandoned_installs(runtimes: &Path) {
    for entry in std::fs::read_dir(runtimes).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|name| name.starts_with('.'))
            .and_then(|name| name.rsplit_once(".installing-"))
            .map(|(_, pid)| pid)
        else {
            continue;
        };
        let alive = pid
            .parse::<u32>()
            .is_ok_and(|pid| Path::new("/proc").join(pid.to_string()).exists());
        if !alive && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
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
    recipes_from(cli, &Paths::from_env()?)
}

fn recipes_from(cli: &Cli, paths: &Paths) -> Result<(PathBuf, Vec<Recipe>)> {
    if let Some(dir) = &cli.titles_dir {
        let recipes = Recipe::load_dir(dir)
            .with_context(|| format!("reading recipes from {}", dir.display()))?;
        return Ok((dir.clone(), recipes));
    }
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

/// The recipes a command that changes the disk goes by.
///
/// A recipe that does not parse stops the command before it does anything:
/// carrying on as if the title had none sent an install to the wrong
/// directory, skipped its setup and wrote a blank recipe over the person's
/// own. No packaged recipes at all is not an error -- an installed tree may
/// ship none -- and then only the person's own are read.
fn recipes_for_change(cli: &Cli, paths: &Paths) -> Result<Vec<Recipe>> {
    let loaded = if let Some(dir) = &cli.titles_dir {
        Recipe::load_dir(dir).with_context(|| format!("reading recipes from {}", dir.display()))
    } else if paths.titles_dir().is_some() {
        layered_recipes_or_the_first_problem(&paths.title_dirs())
    } else {
        layered_recipes_or_the_first_problem(std::slice::from_ref(&paths.user_titles_dir()))
    };
    loaded.context("fix or remove the recipe named above; nothing was changed")
}

/// Every layered recipe, or the first file that could not be read. Commands
/// that only read carry on past such a file with a warning
/// ([`Recipe::load_layered`]); commands that change the disk stop.
fn layered_recipes_or_the_first_problem(dirs: &[PathBuf]) -> Result<Vec<Recipe>> {
    let (recipes, problems) = Recipe::load_layered_report(dirs);
    match problems.into_iter().next() {
        Some(problem) => Err(anyhow!("{problem}")),
        None => Ok(recipes),
    }
}

/// `titles/capabilities.toml`, which turns "missing gameinput.v2" into the
/// message the title would actually have printed. Optional everywhere, so a
/// failure to read it is not worth failing a launch over.
fn registry_for(paths: &Paths) -> Option<Registry> {
    let file = paths.titles_dir()?.join("capabilities.toml");
    Registry::load(&file).ok()
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
    let t_out = tee(out, Arc::clone(&file), std::io::stdout());
    let t_err = tee(err, Arc::clone(&file), std::io::stderr());

    let status = forwarding
        .wait(&mut child)
        .context("waiting for the title")?;
    // The title's process is gone, but Wine started the game in a session of
    // its own, and the prefix's wineserver and system processes outlive it by
    // a few seconds. The service claim is held until they have gone too; a
    // signal now ends them rather than being swallowed.
    forwarding.finish(SESSION_WAIT);
    // Wine processes hold the pipes as long as they run. Whatever is still
    // writing to them after that is not worth holding the launch for.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !(t_out.is_finished() && t_err.is_finished()) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let code = exit_code_from(status.code(), signal_of(&status));
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "=== exited rc={code} ===");
    }
    if code != 0 {
        // Not `eprintln!`: the window that was reading this may have closed,
        // and a failed write must not turn the title's status into a panic.
        let _ = writeln!(
            std::io::stderr(),
            "-- the title exited {code}; the log is at {}",
            log.display()
        );
    }
    Ok(ExitCode::from(code))
}

/// How long a stopped title gets to close by itself, and how long a finished
/// launch waits for the rest of its Wine session.
const SESSION_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Copy a child's output to `terminal` and the log as it arrives.
///
/// Bytes, not text: a title writing its own code page, or anything else that is
/// not UTF-8, used to end the copy at that line, and everything after it was
/// missing from the log the recipes' `log-match` reads. A terminal that went
/// away -- the window closed while the game ran -- is ignored rather than
/// fatal, and the log keeps being written: the pipe is drained to the end
/// either way, so nothing in the title blocks or dies writing to it.
fn tee<R, W>(reader: R, log: Arc<Mutex<std::fs::File>>, terminal: W) -> std::thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
    W: Write + Send + 'static,
{
    std::thread::spawn(move || {
        let mut reader = BufReader::with_capacity(64 * 1024, reader);
        let mut terminal = Some(terminal);
        loop {
            let chunk = match reader.fill_buf() {
                Ok([]) => break,
                Ok(chunk) => chunk,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            // A whole line where there is one, so the two streams interleave
            // in the log by line; a line longer than the buffer goes in parts.
            let length = chunk
                .iter()
                .position(|&b| b == b'\n')
                .map_or(chunk.len(), |at| at + 1);
            let line = &chunk[..length];
            if let Some(out) = terminal.as_mut() {
                if out.write_all(line).and_then(|()| out.flush()).is_err() {
                    terminal = None;
                }
            }
            if let Ok(mut f) = log.lock() {
                let _ = f.write_all(line);
            }
            reader.consume(length);
        }
    })
}

/// The signals that end a launch from outside: the window's Stop (SIGTERM),
/// Ctrl+C (SIGINT), a terminal closing (SIGHUP), and a plain `kill`.
const LAUNCH_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

static SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static SIGNAL_RECEIVED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static FORWARDING: std::sync::OnceLock<Arc<Shared>> = std::sync::OnceLock::new();

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

/// The Wine side of a running title, for ending it.
///
/// Stopping the launch does not stop the game by itself: Wine starts a game in
/// a session of its own, so neither a signal to the launch nor one to its
/// process group reaches it, and a game left running lost its Xbox service
/// when the launch let go of it. So the game is asked to close, the way its
/// window's close button asks, and given [`SESSION_WAIT`] to save and go; the
/// prefix is then ended. A second signal ends it at once.
struct PrefixSession {
    wine: PathBuf,
    wineserver: PathBuf,
    wine_prefix: PathBuf,
    executable: String,
    grace: std::time::Duration,
    running: Box<dyn Fn() -> bool + Send + Sync>,
}

impl PrefixSession {
    fn for_runtime(runtime: &Path, prefix: &Path, executable: &str) -> Self {
        let wine_prefix = prefix.join("pfx");
        let probe = wine_prefix.clone();
        PrefixSession {
            wine: runtime.join("files/bin/wine"),
            wineserver: runtime.join("files/bin/wineserver"),
            executable: executable
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(executable)
                .to_string(),
            wine_prefix,
            grace: SESSION_WAIT,
            running: Box::new(move || wineserver_running(&probe) == Some(true)),
        }
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env("WINEPREFIX", &self.wine_prefix)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// Ask the title's windows to close. `taskkill` without `/F` posts
    /// WM_CLOSE, which is what the close button does: a game saves or asks.
    /// Not waited for here, so the grace period and a second signal both run
    /// from the moment of the first.
    fn request_close(&self) -> Option<std::process::Child> {
        if self.executable.is_empty() {
            return None;
        }
        let mut close = self.command(&self.wine);
        close.args(["taskkill", "/IM", &self.executable]);
        close.spawn().ok()
    }

    /// End every process in the prefix.
    fn kill(&self) {
        let mut kill = self.command(&self.wineserver);
        kill.arg("-k");
        if let Ok(mut child) = kill.spawn() {
            wait_at_most(&mut child, std::time::Duration::from_secs(10));
        }
    }
}

/// What the signal forwarder and the launch share.
struct Shared {
    state: Mutex<ForwardState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct ForwardState {
    title: Option<i32>,
    signals: u32,
    last: i32,
    session: Option<Arc<PrefixSession>>,
    /// The thread stopping the session; it returns whether there was one.
    stopper: Option<std::thread::JoinHandle<bool>>,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Shared {
            state: Mutex::new(ForwardState::default()),
            changed: std::sync::Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ForwardState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A signal arrived: pass it to the title, and start ending its session.
    fn deliver(self: &Arc<Self>, signal: i32) {
        let mut state = self.lock();
        state.signals += 1;
        state.last = signal;
        // Kept across the kill: `wait` clears the pid under the same lock
        // before it reaps the title, so the pid cannot be reaped and reused in
        // between.
        if let Some(pid) = state.title {
            // Safety: an unreaped child of ours while the lock is held.
            unsafe { libc::kill(pid, signal) };
        }
        // A stop under way takes this signal as the second one, and ends the
        // prefix at once. Otherwise this one starts a stop: the first, or one
        // after an earlier stop found no Wine session to end.
        let stopping = state.stopper.as_ref().is_some_and(|s| !s.is_finished());
        if !stopping {
            if let Some(session) = state.session.clone() {
                let shared = Arc::clone(self);
                let start = state.signals;
                state.stopper = Some(std::thread::spawn(move || shared.stop(&session, start)));
            }
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Ask the title to close, give it its grace period, then end the prefix.
    /// Cut short by any signal after the one numbered `start`.
    ///
    /// Nothing is done when the prefix has no Wine session: a signal that came
    /// before Wine was up, or while Proton was still making the prefix, would
    /// otherwise start a session of its own there, outside Proton, and then
    /// wait the whole grace period for it.
    fn stop(&self, session: &PrefixSession, start: u32) -> bool {
        if !(session.running)() {
            return false;
        }
        let deadline = std::time::Instant::now() + session.grace;
        let mut close = session.request_close();
        let mut state = self.lock();
        while state.signals <= start && (session.running)() {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            if let Some(child) = &mut close {
                if !matches!(child.try_wait(), Ok(None)) {
                    close = None;
                }
            }
            let wait = (deadline - now).min(std::time::Duration::from_millis(200));
            state = self
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(state);
        if let Some(mut child) = close {
            let _ = child.kill();
            let _ = child.wait();
        }
        if (session.running)() {
            session.kill();
        }
        true
    }
}

/// Signals that would end a launch, passed on to its title instead.
///
/// Ended by a signal, this process never unwinds: its claim on the service is
/// never dropped, and the service is never stopped. Caught, the signal goes to
/// the title, and its Wine session is ended (see [`PrefixSession`]); the
/// launch then finishes the ordinary way, and the last title out still stops
/// the service. Handlers do not survive exec, so every child starts with the
/// default dispositions.
struct Forwarding {
    shared: Arc<Shared>,
    installed: bool,
}

impl Forwarding {
    fn install() -> Result<Self> {
        if let Some(shared) = FORWARDING.get() {
            return Ok(Self {
                shared: Arc::clone(shared),
                installed: true,
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

        let shared = Shared::new();
        let forwarded = Arc::clone(&shared);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut byte = [0u8];
            loop {
                match read.read(&mut byte) {
                    Ok(1) => forwarded.deliver(libc::c_int::from(byte[0])),
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
        let _ = FORWARDING.set(Arc::clone(&shared));
        Ok(Self {
            shared,
            installed: true,
        })
    }

    /// The last signal caught, if any.
    fn received(&self) -> Option<i32> {
        let last = self.shared.lock().last;
        let caught = if self.installed {
            SIGNAL_RECEIVED.load(std::sync::atomic::Ordering::SeqCst)
        } else {
            0
        };
        Some(if last != 0 { last } else { caught }).filter(|s| *s != 0)
    }

    /// The Wine session a signal should end, once there is one.
    fn set_session(&self, session: Option<Arc<PrefixSession>>) {
        self.shared.lock().session = session;
    }

    /// Wait for the title, passing signals on to it meanwhile.
    fn wait(&self, child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
        let pid = child.id() as i32;
        {
            let mut state = self.shared.lock();
            state.title = Some(pid);
            // One that came before there was a title to pass it to.
            let early = if state.last != 0 {
                state.last
            } else if self.installed {
                SIGNAL_RECEIVED.load(std::sync::atomic::Ordering::SeqCst)
            } else {
                0
            };
            if early != 0 {
                // Safety: our own child, not yet reaped.
                unsafe { libc::kill(pid, early) };
            }
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
        self.shared.lock().title = None;
        child.wait()
    }

    /// After the title: let a stop that is under way finish, or wait up to
    /// `limit` for the rest of the Wine session to end by itself. A signal
    /// meanwhile starts a stop, and a second one cuts it short.
    fn finish(&self, limit: std::time::Duration) {
        let deadline = std::time::Instant::now() + limit;
        let mut state = self.shared.lock();
        loop {
            // A stop under way is waited for whatever `limit` says: it is
            // bounded by its own grace period, and a signal meanwhile has to
            // find it still there to cut it short.
            if let Some(stopper) = &state.stopper {
                if !stopper.is_finished() {
                    state = self
                        .shared
                        .changed
                        .wait_timeout(state, std::time::Duration::from_millis(100))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                    continue;
                }
                // One that ended a session is the end of the launch; one that
                // found none leaves the wait for a session to go on.
                if let Some(stopper) = state.stopper.take() {
                    if stopper.join().unwrap_or(true) {
                        return;
                    }
                }
            }
            let Some(session) = state.session.clone() else {
                return;
            };
            let now = std::time::Instant::now();
            if now >= deadline || !(session.running)() {
                return;
            }
            let wait = (deadline - now).min(std::time::Duration::from_millis(200));
            state = self
                .shared
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
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
        // A service started for one title identity is never reused for
        // another: the binding names the product, content and title.
        let binding = format!(
            "{}:{}:{}:{}:{}",
            id.trim(),
            plan.get("XODUS_STORE_PRODUCT").unwrap_or_default(),
            plan.get("XODUS_STORE_CONTENT").unwrap_or_default(),
            plan.get("XODUS_TITLE_ID").unwrap_or_default(),
            plan.get("XODUS_TITLE_CLIENT_ID").unwrap_or_default()
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

/// What a launch tells the service it starts, each set or removed so nothing
/// leaks in from the launcher's own environment.
///
/// - `XODUS_STORE_PROFILE` / `XODUS_PLAYER_PROFILE`: the account pair.
/// - `XODUS_STORE_PRODUCT` / `XODUS_STORE_CONTENT`: the Store product and
///   content the service licenses, and the only product it signs licence
///   tokens for, with the add-ons and editions the Store catalog ties to it.
/// - `XODUS_TITLE_ID` / `XODUS_TITLE_CLIENT_ID`: the title's `TitleId`
///   (decimal) and `MSAAppId` out of its `MicrosoftGame.config`. The service
///   mints XSTS tokens claiming this title or none, and signs licence requests
///   as it whatever a request says. Empty means the config names no title, so
///   no request may claim one. Left unset only by older launchers, whose
///   services then take the title on a request's word as before; a launch with
///   account roles needs a service that understands them
///   (`accounts::SERVICE_TITLE_SCOPE`).
const SERVICE_BINDING: [&str; 6] = [
    "XODUS_STORE_PROFILE",
    "XODUS_PLAYER_PROFILE",
    "XODUS_STORE_PRODUCT",
    "XODUS_STORE_CONTENT",
    "XODUS_TITLE_ID",
    "XODUS_TITLE_CLIENT_ID",
];

/// Bind the launch's service to one title: its Store product and content,
/// and the identity the runtime will read out of the same install's config.
fn bind_service_title(
    plan: &mut launch::Plan,
    product_id: &str,
    content: &str,
    game_dir: &Path,
) -> Result<()> {
    // Upper case like every Store id the runtime sends; the service
    // compares the bound product exactly.
    plan.env_set.insert(
        "XODUS_STORE_PRODUCT".into(),
        product_id.to_ascii_uppercase(),
    );
    plan.env_set
        .insert("XODUS_STORE_CONTENT".into(), content.to_string());
    // A package without a readable config has no title for a request to
    // claim, so it is bound to none rather than left to choose.
    let identity = install::title_identity(game_dir).unwrap_or_default();
    // The service refuses to start on anything else; say why here instead.
    if !identity
        .client_id
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"-{}".contains(&c))
    {
        bail!(
            "{}'s MicrosoftGame.config has an MSAAppId the Xbox service cannot be bound to",
            game_dir.display()
        );
    }
    plan.env_set
        .insert("XODUS_TITLE_ID".into(), identity.title_id);
    plan.env_set
        .insert("XODUS_TITLE_CLIENT_ID".into(), identity.client_id);
    Ok(())
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
        // The pair and the title this launch was resolved for, and nothing
        // inherited.
        for key in SERVICE_BINDING {
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
fn run_setup(recipe: &Recipe, when: &str, scripts_dir: &Path, game_dir: &Path) -> Result<()> {
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
                let script = scripts_dir.join("fix-xcurl.sh");
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
            checks.push(Check::ok("client", "xodus-cli").with_detail(path.display().to_string()));
            // A failure: without these the loader refuses the client for
            // every command, so nothing signs in, lists or launches.
            checks.push(match ferestre_core::client_deps::check(&path) {
                Ok(None) => Check::ok("client", "Xbox client libraries available"),
                Ok(Some(missing)) => {
                    Check::fail("client", "the Xbox client cannot start: missing libraries")
                        .with_detail(missing.libraries.join(", "))
                        .with_hint(missing.hint())
                }
                Err(error) => Check::warn("client", "could not check the Xbox client's libraries")
                    .with_detail(format!("{error:#}")),
            });
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

    // The runtime a launch would pick first: managed runtimes included, which
    // `discover` alone never saw, so a machine whose only runtime came from
    // `install-runtime` was told it had none.
    let found = match runtime::all(paths).into_iter().next() {
        Some(rt) => Ok(rt),
        None => InstalledRuntime::discover(paths),
    };
    match found {
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

    // Every directory a launch reads, the person's own included: a file there
    // that does not parse stops every command that loads recipes.
    let dirs: Vec<PathBuf> = paths
        .title_dirs()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect();
    if paths.titles_dir().is_none() && dirs.is_empty() {
        checks.push(
            Check::warn("recipes", "no title recipes found")
                .with_hint("set XODUS_REPO_DIR, or pass --titles-dir"),
        );
    } else {
        let shown = dirs
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        checks.push(match layered_recipes_or_the_first_problem(&dirs) {
            Ok(recipes) => Check::ok("recipes", format!("{} title recipe(s)", recipes.len()))
                .with_detail(shown),
            Err(err) => Check::fail("recipes", "a recipe does not parse")
                .with_detail(first_line(&err.to_string()))
                .with_hint(
                    "fix or remove that file; until then, install and uninstall refuse to act",
                ),
        });
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

/// The plan as JSON, with account identifiers in its environment masked the
/// way `ferestre env` masks them.
fn plan_json(plan: &launch::Plan, show_account: bool) -> Value {
    let env_set: serde_json::Map<String, Value> = plan
        .env_set
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                Value::String(env_display(name, value, show_account)),
            )
        })
        .collect();
    json!({
        "program": plan.program.display().to_string(),
        "args": plan.args,
        "env_set": env_set,
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
/// a description of it. Account identifiers -- `XGR_XUID`, the Store and Player
/// profiles -- are masked unless `--show-account` is given, like everywhere
/// else, so the output stays safe to paste into a bug report; with the flag it
/// runs as printed.
fn render_plan(plan: &launch::Plan, show_account: bool) -> String {
    let mut out = String::new();
    for name in &plan.env_unset {
        out.push_str(&format!("unset {name}\n"));
    }
    for (name, value) in &plan.env_set {
        out.push_str(&format!(
            "export {name}={}\n",
            shell_quote(&env_display(name, value, show_account))
        ));
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

    /// The service learns its title from the launch, out of the install's own
    /// config, and every variable the binding sets reaches the service.
    #[test]
    fn a_bound_service_is_told_the_title_the_runtime_will_name() {
        let scratch = Scratch::new("service-title");
        let plan = |config: Option<&str>| {
            let game = scratch.root.join("game");
            let _ = std::fs::remove_dir_all(&game);
            std::fs::create_dir_all(&game).unwrap();
            if let Some(config) = config {
                std::fs::write(game.join("MicrosoftGame.config"), config).unwrap();
            }
            let mut plan = launch::Plan {
                program: PathBuf::from("/opt/xodus/xodus-cli"),
                args: vec![],
                env_set: Default::default(),
                env_unset: vec![],
                cwd: None,
            };
            bind_service_title(
                &mut plan,
                "9p5786pjb9rp",
                "00000000-0000-0000-0000-000000000001",
                &game,
            )
            .map(|()| plan)
        };
        let bound = plan(Some(
            "<Game><TitleId>6B9DE498</TitleId><MSAAppId>00000000-0000-0000-0000-00004C26E6C9</MSAAppId></Game>",
        ))
        .unwrap();
        assert_eq!(bound.get("XODUS_STORE_PRODUCT"), Some("9P5786PJB9RP"));
        assert_eq!(
            bound.get("XODUS_STORE_CONTENT"),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(bound.get("XODUS_TITLE_ID"), Some("1805509784"));
        assert_eq!(
            bound.get("XODUS_TITLE_CLIENT_ID"),
            Some("00000000-0000-0000-0000-00004C26E6C9")
        );
        for key in bound.env_set.keys() {
            assert!(
                SERVICE_BINDING.contains(&key.as_str()),
                "{key} never reaches the service"
            );
        }
        // No config, or one naming no title: bound to no title, not unbound.
        for config in [None, Some("<Game><MSAAppId>client</MSAAppId></Game>")] {
            let none = plan(config).unwrap();
            assert_eq!(none.get("XODUS_TITLE_ID"), Some(""));
            assert_eq!(none.get("XODUS_TITLE_CLIENT_ID"), Some(""));
        }
        assert!(plan(Some("<TitleId>1</TitleId><MSAAppId>a b;c</MSAAppId>")).is_err());
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
        let out = render_plan(&plan, false);
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

    // -- fixtures for whole commands ---------------------------------------

    /// An exit code, comparable: `ExitCode` has no `PartialEq` before Rust
    /// 1.79, and the launcher builds with 1.77.
    fn code(code: ExitCode) -> String {
        format!("{code:?}")
    }

    /// A home directory of its own: games, state, config and cache under one
    /// temporary root, a stand-in Xbox client, scripts and recipes, and
    /// `Paths` resolved from exactly that and nothing on the host.
    struct Home {
        scratch: Scratch,
        extra: Vec<(String, String)>,
    }

    impl Home {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            for dir in [
                "home",
                "games",
                "client",
                "scripts",
                "titles",
                "config/ferestre",
            ] {
                std::fs::create_dir_all(scratch.root.join(dir)).unwrap();
            }
            let home = Home {
                scratch,
                extra: Vec::new(),
            };
            home.client("exit 0");
            home
        }

        fn root(&self) -> &Path {
            &self.scratch.root
        }

        fn games(&self) -> PathBuf {
            self.root().join("games")
        }

        fn paths(&self) -> Paths {
            let root = self.root();
            let mut vars: Vec<(String, String)> = [
                ("HOME", "home"),
                ("XODUS_GAMES_DIR", "games"),
                ("XODUS_CLI_DIR", "client"),
                ("FERESTRE_SCRIPTS_DIR", "scripts"),
                ("FERESTRE_TITLES", "titles"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_CACHE_HOME", "cache"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), root.join(v).display().to_string()))
            .collect();
            vars.extend(self.extra.iter().cloned());
            Paths::resolve(&ferestre_core::paths::Env::from_map(vars)).unwrap()
        }

        /// The stand-in client: `$1` is the command, `$3` the destination.
        fn client(&self, body: &str) {
            let script = self.root().join("client/xodus-cli");
            std::fs::write(
                &script,
                format!("#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/calls\"\n{body}\n"),
            )
            .unwrap();
            make_executable(&script);
        }

        fn client_ran(&self) -> bool {
            self.root().join("client/calls").exists()
        }

        fn recipe(&self, dir: &str, file: &str, text: &str) -> PathBuf {
            let dir = self.root().join(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(file);
            std::fs::write(&path, text).unwrap();
            path
        }

        fn script(&self, name: &str, body: &str) {
            let script = self.root().join("scripts").join(name);
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            make_executable(&script);
        }

        /// A runtime as `install-runtime` leaves one: in the state directory,
        /// publishing `provides`, or probed when that is `None`.
        fn runtime(&self, version: &str, provides: Option<&[&str]>) -> PathBuf {
            let dir = self.root().join("state/ferestre/runtimes").join(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("proton"), "#!/bin/sh\n").unwrap();
            std::fs::write(dir.join("version"), format!("1 {version}\n")).unwrap();
            if let Some(provides) = provides {
                let manifest = dir.join(runtime::MANIFEST_PATH);
                std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
                std::fs::write(manifest, serde_json::to_string(provides).unwrap()).unwrap();
            }
            dir
        }
    }

    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn cli() -> Cli {
        Cli {
            command: Cmd::Version,
            json: false,
            titles_dir: None,
            show_account: false,
            no_color: true,
        }
    }

    const GAME: &str = "9ZZTESTGAME1";

    fn game_recipe(extra: &str) -> String {
        format!(
            r#"schema = 1
[title]
product-id = "{GAME}"
name = "Test Game"
slug = "testgame"
[launch]
executable = 'Game.exe'
[status]
state = "playable"
summary = "Runs."
{extra}"#
        )
    }

    /// A finished install: the manifest, the container, and the config naming
    /// the product.
    fn installed_package(dir: &Path, product_id: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("appxmanifest.xml"),
            r#"<Package><Identity Name="Test" Version="1.0.0.0"/></Package>"#,
        )
        .unwrap();
        std::fs::write(dir.join(STREAMING_CONTAINER), b"old container").unwrap();
        std::fs::write(
            dir.join("MicrosoftGame.config"),
            format!("<Game><StoreId>{product_id}</StoreId></Game>"),
        )
        .unwrap();
    }

    fn record(paths: &Paths, product_id: &str, dir: &Path) {
        install::save(
            paths.state_dir(),
            &install::Record {
                product_id: product_id.into(),
                dir: dir.to_path_buf(),
                content_ids: vec!["content".into()],
                installed_at: Some("2020-01-01T00:00:00Z".into()),
                package_version: Some("1.0.0.0".into()),
            },
        )
        .unwrap();
    }

    fn uninstall(paths: &Paths, title: &str, removal: Removal) -> Result<ExitCode> {
        uninstall_title(&cli(), paths, title, removal, &mut std::io::empty())
    }

    const YES: Removal = Removal {
        keep_files: false,
        remove_prefix: false,
        yes: true,
    };

    // -- uninstall ----------------------------------------------------------

    /// `uninstall "" --yes` removed the games directory with every prefix in
    /// it, and `uninstall .. --yes` emptied the home directory.
    #[test]
    fn uninstall_takes_only_a_product_id_or_a_recipe_slug() {
        let home = Home::new("uninstall-ids");
        let paths = home.paths();
        installed_package(&home.games().join("other"), "9ZZOTHERGAME");
        std::fs::write(home.root().join("home/keep"), b"precious").unwrap();
        for title in ["", ".", "..", "../home", "/", "9ZZ/../..", "testgame"] {
            let error = uninstall(&paths, title, YES).unwrap_err();
            assert_eq!(exit_code_for(&error), EXIT_USAGE, "{title:?}: {error}");
        }
        assert!(home.games().join("other/appxmanifest.xml").exists());
        assert!(home.root().join("home/keep").exists());

        // A slug a recipe gives is fine, and means that recipe's product.
        home.recipe("titles", "game.toml", &game_recipe(""));
        let dir = home.games().join("testgame");
        installed_package(&dir, GAME);
        assert_eq!(
            code(uninstall(&paths, "testgame", YES).unwrap()),
            code(ExitCode::SUCCESS)
        );
        assert!(!dir.exists());
        assert!(home.games().join("other/appxmanifest.xml").exists());
    }

    /// A record is a file anybody can edit, and an install directory used to be
    /// whatever was typed: the library, the home directory or a folder of
    /// other titles is never deleted on its say-so.
    #[test]
    fn uninstall_never_deletes_a_library_or_the_launchers_own_directories() {
        let home = Home::new("uninstall-guard");
        let paths = home.paths();
        let games = home.games();
        installed_package(&games.join("other"), "9ZZOTHERGAME");
        record(&paths, "9ZZOTHERGAME", &games.join("other"));
        std::fs::create_dir_all(games.join("other-proton/pfx")).unwrap();

        let library = home.root().join("library");
        installed_package(&library, GAME);
        std::fs::create_dir_all(library.join("testgame-proton/pfx")).unwrap();
        // A folder holding another title's recorded install.
        let shelf = home.root().join("shelf");
        installed_package(&shelf.join("third"), "9ZZTHIRDGAME");
        record(&paths, "9ZZTHIRDGAME", &shelf.join("third"));
        for dir in [
            games.clone(),
            games.join(".."),
            home.root().join("home"),
            home.root().join("state/ferestre"),
            library.clone(),
            shelf.clone(),
        ] {
            record(&paths, GAME, &dir);
            let result = uninstall(&paths, GAME, YES);
            assert!(result.is_err(), "{} was accepted", dir.display());
        }
        assert!(games.join("other/appxmanifest.xml").exists());
        assert!(library.join("appxmanifest.xml").exists());
        assert!(shelf.join("third/appxmanifest.xml").exists());

        // The title's own folder goes, with a folder of its own that happens
        // to be called `players`.
        let own = games.join("testgame");
        installed_package(&own, GAME);
        std::fs::create_dir_all(own.join("players/data")).unwrap();
        record(&paths, GAME, &own);
        assert_eq!(
            code(uninstall(&paths, GAME, YES).unwrap()),
            code(ExitCode::SUCCESS)
        );
        assert!(!own.exists());
        assert!(install::load(paths.state_dir(), GAME).is_none());
    }

    /// With account roles each Player plays from their own prefix, which
    /// `--prefix` used to leave behind without a word.
    #[test]
    fn uninstall_names_and_removes_every_players_prefix() {
        let home = Home::new("uninstall-prefixes");
        home.recipe("titles", "game.toml", &game_recipe(""));
        let paths = home.paths();
        let games = home.games();
        installed_package(&games.join("testgame"), GAME);
        let base = games.join("testgame-proton");
        let player = games
            .join("players")
            .join("a".repeat(64))
            .join("testgame-proton");
        let unrelated = games
            .join("players")
            .join("b".repeat(64))
            .join("other-proton");
        for dir in [&base, &player, &unrelated] {
            std::fs::create_dir_all(dir.join("pfx")).unwrap();
        }
        assert_eq!(title_prefixes(&base), vec![base.clone(), player.clone()]);

        // Kept unless asked.
        uninstall(&paths, GAME, YES).unwrap();
        assert!(base.exists() && player.exists());
        installed_package(&games.join("testgame"), GAME);
        uninstall(
            &paths,
            GAME,
            Removal {
                remove_prefix: true,
                ..YES
            },
        )
        .unwrap();
        assert!(!base.exists());
        assert!(!player.exists());
        assert!(unrelated.exists());
    }

    /// Asked once for everything, and a "no" leaves everything.
    #[test]
    fn uninstall_asks_before_deleting_the_prefix_too() {
        let home = Home::new("uninstall-ask");
        home.recipe("titles", "game.toml", &game_recipe(""));
        let paths = home.paths();
        let prefix = home.games().join("testgame-proton");
        std::fs::create_dir_all(&prefix).unwrap();
        record(&paths, GAME, &home.games().join("gone"));
        let removal = Removal {
            keep_files: false,
            remove_prefix: true,
            yes: false,
        };
        let mut no = std::io::Cursor::new(b"n\n".to_vec());
        let answered = uninstall_title(&cli(), &paths, GAME, removal, &mut no).unwrap();
        assert_eq!(code(answered), code(ExitCode::FAILURE));
        assert!(prefix.exists());
        let mut yes = std::io::Cursor::new(b"y\n".to_vec());
        uninstall_title(&cli(), &paths, GAME, removal, &mut yes).unwrap();
        assert!(!prefix.exists());
    }

    // -- install ------------------------------------------------------------

    /// "Install to" is a library: the title gets its own folder in it, and its
    /// own folder chosen again is used as it is.
    #[test]
    fn an_install_folder_is_a_library_the_title_gets_a_folder_in() {
        let home = Home::new("destination");
        let recipe = Recipe::parse(&game_recipe("[install]\ndir = \"testgame/package\"")).unwrap();
        let paths = home.paths();
        let library = home.root().join("library");
        std::fs::create_dir_all(library.join("someone-elses-files")).unwrap();

        let dest = install_destination(&paths, GAME, Some(&recipe), Some(&library)).unwrap();
        assert_eq!(dest, library.join("testgame/package"));
        let dest = install_destination(&paths, "9ZZNORECIPE1", None, Some(&library)).unwrap();
        assert_eq!(dest, library.join("9zznorecipe1"));

        // `~` is the home directory, not a folder called `~`.
        let dest = install_destination(&paths, GAME, None, Some(Path::new("~/Games"))).unwrap();
        assert_eq!(
            dest,
            home.root().join("home/Games").join(GAME.to_lowercase())
        );
        assert!(install_destination(&paths, GAME, None, Some(Path::new("~bob/x"))).is_err());
        // Relative means from here, and is kept absolute.
        let dest = install_destination(&paths, GAME, None, Some(Path::new("lib"))).unwrap();
        assert!(dest.is_absolute(), "{}", dest.display());
        assert_eq!(
            dest,
            std::env::current_dir()
                .unwrap()
                .join("lib")
                .join(GAME.to_lowercase())
        );

        // The title's own folder, by its package or by its record, is used
        // for a repair or an update.
        let own = home.root().join("elsewhere/my-game");
        installed_package(&own, GAME);
        assert_eq!(
            install_destination(&paths, GAME, Some(&recipe), Some(&own)).unwrap(),
            own
        );
        let recorded = home.root().join("recorded");
        std::fs::create_dir_all(&recorded).unwrap();
        record(&paths, GAME, &recorded);
        assert_eq!(
            install_destination(&paths, GAME, Some(&recipe), Some(&recorded)).unwrap(),
            recorded
        );
        // The folder the title gets in the games directory, which the window
        // offers, is its own folder too, not a library to nest it in.
        let default = home.games().join("testgame/package");
        assert_eq!(
            install_destination(&paths, GAME, Some(&recipe), Some(&default)).unwrap(),
            default
        );
        let default = home.games().join("9zznorecipe1");
        assert_eq!(
            install_destination(&paths, "9ZZNORECIPE1", None, Some(&default)).unwrap(),
            default
        );
        // Another title's package is not overwritten.
        installed_package(&library.join("9zzotherone1"), "9ZZOTHERGAME");
        let error = install_destination(&paths, "9ZZOTHERONE1", None, Some(&library)).unwrap_err();
        assert!(error.to_string().contains("9ZZOTHERGAME"), "{error}");
        // Nor are somebody's own files that happen to be where the title's
        // folder would go: removing the title would remove them too.
        std::fs::create_dir_all(library.join("9zznotes0001")).unwrap();
        std::fs::write(library.join("9zznotes0001/notes.txt"), "mine").unwrap();
        assert!(install_destination(&paths, "9ZZNOTES0001", None, Some(&library)).is_err());
        // A first download that was stopped before its package said whose it
        // is carries on where it is.
        let partial = library.join("9zzpartial01");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join(install::PARTIAL_CONTAINER), b"").unwrap();
        assert_eq!(
            install_destination(&paths, "9ZZPARTIAL01", None, Some(&library)).unwrap(),
            partial
        );
    }

    /// An update of a title with no recipe goes where it was installed.
    #[test]
    fn an_update_without_a_recipe_goes_to_the_recorded_folder() {
        let home = Home::new("destination-record");
        let paths = home.paths();
        let elsewhere = home.root().join("ssd/game");
        installed_package(&elsewhere, GAME);
        record(&paths, GAME, &elsewhere);
        assert_eq!(
            install_destination(&paths, GAME, None, None).unwrap(),
            elsewhere
        );
    }

    fn install(home: &Home, dir: Option<&Path>) -> Result<ExitCode> {
        install_title(
            &cli(),
            &home.paths(),
            GAME,
            dir,
            &cancel::Cancellation::default(),
        )
    }

    /// The client gives up on an update -- a lapsed licence, a full disk --
    /// and still exits zero. That used to be recorded as a fresh install.
    #[test]
    fn an_update_the_client_did_not_finish_is_a_failure_and_not_recorded() {
        let home = Home::new("update-unfinished");
        home.recipe("titles", "game.toml", &game_recipe(""));
        let paths = home.paths();
        let dir = home.games().join("testgame");
        installed_package(&dir, GAME);
        record(&paths, GAME, &dir);
        let before = install::load(paths.state_dir(), GAME).unwrap();

        // Stopped partway: its working container is still there.
        home.client("touch \"$3/.xodus-streaming-tmp.msixvc\"");
        assert_eq!(code(install(&home, None).unwrap()), code(ExitCode::FAILURE));
        assert_eq!(install::load(paths.state_dir(), GAME).unwrap(), before);
        std::fs::remove_file(dir.join(install::PARTIAL_CONTAINER)).unwrap();

        // Did nothing at all.
        home.client("exit 0");
        assert_eq!(code(install(&home, None).unwrap()), code(ExitCode::FAILURE));
        assert_eq!(install::load(paths.state_dir(), GAME).unwrap(), before);

        // A finished run replaces the container.
        home.client(
            "printf new > \"$3/.xodus-streaming-tmp.msixvc\" && \
             mv \"$3/.xodus-streaming-tmp.msixvc\" \"$3/.xodus-streaming.msixvc\"",
        );
        assert_eq!(code(install(&home, None).unwrap()), code(ExitCode::SUCCESS));
        assert_ne!(install::load(paths.state_dir(), GAME).unwrap(), before);
    }

    /// The after-install step needs the scripts, not a runtime: a machine
    /// whose only runtime came from `install-runtime` failed every install at
    /// the very end. And a step that fails leaves the install recorded.
    #[test]
    fn after_install_setup_runs_without_a_runtime_and_a_failure_keeps_the_record() {
        let home = Home::new("after-install");
        home.recipe(
            "titles",
            "game.toml",
            &game_recipe("[[setup]]\naction = \"substitute-xcurl\"\nwhen = \"after-install\"\n"),
        );
        let paths = home.paths();
        assert!(paths.runtime_dir().is_none());
        home.client(
            "mkdir -p \"$3\" && printf m > \"$3/appxmanifest.xml\" && \
             printf c > \"$3/.xodus-streaming.msixvc\"",
        );
        home.script("fix-xcurl.sh", "touch \"$1/patched\"");
        assert_eq!(code(install(&home, None).unwrap()), code(ExitCode::SUCCESS));
        let dir = home.games().join("testgame");
        assert!(dir.join("patched").exists());
        assert!(install::load(paths.state_dir(), GAME).is_some());

        std::fs::remove_dir_all(&dir).unwrap();
        install::forget(paths.state_dir(), GAME).unwrap();
        home.script("fix-xcurl.sh", "exit 3");
        assert_eq!(code(install(&home, None).unwrap()), code(ExitCode::FAILURE));
        assert!(install::load(paths.state_dir(), GAME).is_some());
    }

    /// One recipe that does not parse stops an install before it starts,
    /// instead of sending it to the wrong place and writing a blank recipe
    /// over the person's own.
    #[test]
    fn a_recipe_that_does_not_parse_stops_install_and_uninstall_before_anything_happens() {
        let home = Home::new("broken-recipe");
        let own = home.recipe(
            "config/ferestre/titles",
            &format!("{GAME}.toml"),
            &game_recipe(""),
        );
        let edited = std::fs::read_to_string(&own).unwrap();
        home.recipe("config/ferestre/titles", "broken.toml", "schema = [");
        let error = install(&home, None).unwrap_err();
        assert!(format!("{error:#}").contains("broken.toml"), "{error:#}");
        assert!(!home.client_ran());
        assert_eq!(std::fs::read_to_string(&own).unwrap(), edited);
        assert!(uninstall(&home.paths(), GAME, YES).is_err());
    }

    /// A detected recipe never replaces a file that is already there.
    #[test]
    fn a_detected_recipe_is_never_written_over_an_existing_file() {
        let home = Home::new("detected-existing");
        let paths = home.paths();
        let existing = home.recipe("config/ferestre/titles", &format!("{GAME}.toml"), "mine");
        let mut recipe = Recipe::blank(GAME, "Test");
        recipe.launch.executable = "Game.exe".into();
        assert!(save_detected_recipe(&paths, &recipe).is_err());
        assert_eq!(std::fs::read_to_string(existing).unwrap(), "mine");
    }

    /// A launch holds its title shared; install, update and remove need it
    /// to themselves, and say so instead of waiting.
    #[test]
    fn a_running_title_is_not_installed_over_or_removed() {
        let home = Home::new("title-lock");
        home.recipe("titles", "game.toml", &game_recipe(""));
        let paths = home.paths();
        let first = TitleLock::take(&paths, GAME, LockMode::Run).unwrap();
        let second = TitleLock::take(&paths, GAME, LockMode::Run).unwrap();
        assert!(first.is_some() && second.is_some());
        let error = install(&home, None).unwrap_err();
        assert!(error.to_string().contains("running"), "{error}");
        assert!(!home.client_ran());
        let dir = home.games().join("testgame");
        installed_package(&dir, GAME);
        assert!(uninstall(&paths, GAME, YES).is_err());
        assert!(dir.exists());
        drop((first, second));

        let change = TitleLock::take(&paths, GAME, LockMode::Change).unwrap();
        let error = TitleLock::take(&paths, GAME, LockMode::Run)
            .err()
            .expect("a launch waits for nothing while an install runs");
        assert!(error.to_string().contains("being installed"), "{error}");
        drop(change);
        assert!(TitleLock::take(&paths, GAME, LockMode::Run).is_ok());
    }

    // -- choosing a runtime -------------------------------------------------

    fn installed_runtime(
        name: &str,
        provides: &[&str],
        source: CapabilitySource,
    ) -> InstalledRuntime {
        InstalledRuntime {
            path: PathBuf::from(format!("/runtimes/{name}")),
            version: Some(name.into()),
            provides: names(provides),
            source,
        }
    }

    /// The gate decides, so it can say what is missing, and `--force` and a
    /// probed runtime work.
    #[test]
    fn a_runtime_missing_a_requirement_is_still_chosen_for_the_gate_to_judge() {
        let recipe = recipe();
        let full = installed_runtime(
            "full",
            &["loader.memfd-main-image", "gameinput.v2"],
            CapabilitySource::Manifest,
        );
        let newer = installed_runtime(
            "newer",
            &["loader.memfd-main-image"],
            CapabilitySource::Manifest,
        );
        // The newest that satisfies, first.
        let runtimes = [newer.clone(), full.clone()];
        assert_eq!(
            choose_runtime(&recipe, &runtimes, None, None).unwrap(),
            &full
        );
        // Then the newest at all, refused with the reason.
        let runtimes = [newer.clone()];
        let chosen = choose_runtime(&recipe, &runtimes, None, None).unwrap();
        let assessment = runtime::assess(&recipe, chosen, None);
        assert_eq!(gate(&assessment), Gate::Refused);
        assert!(refusal(&assessment, chosen).contains("gameinput.v2"));
        // A probed one launches unverified.
        let probed = [installed_runtime(
            "probed",
            &["loader.memfd-main-image"],
            CapabilitySource::Probed,
        )];
        let chosen = choose_runtime(&recipe, &probed, None, None).unwrap();
        assert_eq!(
            gate(&runtime::assess(&recipe, chosen, None)),
            Gate::Unverified
        );
        // A chosen runtime is used even if it falls short, and named if gone.
        let runtimes = [full.clone(), newer.clone()];
        assert_eq!(
            choose_runtime(&recipe, &runtimes, Some(&newer.path), None).unwrap(),
            &newer
        );
        assert!(choose_runtime(&recipe, &runtimes, Some(Path::new("/gone")), None).is_err());
        assert!(choose_runtime(&recipe, &[], None, None).is_err());
    }

    /// The whole of `run`'s decision, with a runtime `install-runtime` would
    /// have left: refused with the missing capability named, and launched
    /// with `--force`.
    #[test]
    fn run_refuses_with_the_reason_and_force_gets_past_it() {
        let mut home = Home::new("run-force");
        home.recipe(
            "titles",
            "game.toml",
            &game_recipe("[runtime]\nrequires = [\"loader.memfd-main-image\", \"gameinput.v2\"]\n"),
        );
        home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
        home.extra
            .push(("XGR_XUID".into(), "2533274812345678".into()));
        let paths = home.paths();
        let request = LaunchRequest {
            dry_run: true,
            ..LaunchRequest::default()
        };
        let error = prepare_launch(&cli(), &paths, "testgame", request)
            .err()
            .expect("refused");
        let message = error.to_string();
        assert!(message.contains("gameinput.v2"), "{message}");
        assert!(message.contains("--force"), "{message}");

        let forced = LaunchRequest {
            force: true,
            ..request
        };
        let prepared = prepare_launch(&cli(), &paths, "testgame", forced).unwrap();
        assert!(prepared.runtime.path.ends_with("ferestre-11.0.2"));
        // What a dry run prints is safe to paste.
        let text = render_plan(&prepared.plan, false);
        assert!(!text.contains("2533274812345678"), "{text}");
        assert!(text.contains("XGR_XUID='************5678'"), "{text}");
        let doc = plan_json(&prepared.plan, false).to_string();
        assert!(!doc.contains("2533274812345678"), "{doc}");
        assert!(render_plan(&prepared.plan, true).contains("2533274812345678"));
    }

    /// A download that stopped partway is part old build, part new and part
    /// missing; it is not launched, and nothing is started for it.
    #[test]
    fn an_unfinished_download_is_not_launched() {
        let home = Home::new("run-unfinished");
        home.recipe("titles", "game.toml", &game_recipe(""));
        home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
        let paths = home.paths();
        let dir = home.games().join("testgame");
        installed_package(&dir, GAME);
        std::fs::write(dir.join(install::PARTIAL_CONTAINER), b"").unwrap();
        let prepared =
            prepare_launch(&cli(), &paths, "testgame", LaunchRequest::default()).unwrap();
        let error = launch_prepared(&paths, prepared, false).expect_err("refused");
        assert!(
            error.to_string().contains("did not finish downloading"),
            "{error}"
        );
        assert!(!home.games().join("testgame-proton").exists());
    }

    /// `run` itself takes the title's lock: a launch while an install or
    /// removal of the title is under way is refused before anything starts.
    #[test]
    fn a_title_being_installed_is_not_launched() {
        let home = Home::new("run-locked");
        home.recipe("titles", "game.toml", &game_recipe(""));
        home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
        let paths = home.paths();
        let dir = home.games().join("testgame");
        installed_package(&dir, GAME);
        // Without the lock, the launch would stop here instead, still before
        // anything is started.
        std::fs::write(dir.join(install::PARTIAL_CONTAINER), b"").unwrap();
        let prepared =
            prepare_launch(&cli(), &paths, "testgame", LaunchRequest::default()).unwrap();
        let change = TitleLock::take(&paths, GAME, LockMode::Change).unwrap();
        let error = launch_prepared(&paths, prepared, false).expect_err("refused");
        assert!(error.to_string().contains("being installed"), "{error}");
        assert!(!home.games().join("testgame-proton").exists());
        drop(change);
    }

    /// The launch log is named by the recipe's slug, which is data: it goes
    /// through the games directory's containment check, not around it.
    #[test]
    fn a_slug_cannot_put_the_launch_log_outside_the_games_directory() {
        let home = Home::new("run-log");
        home.recipe(
            "titles",
            "game.toml",
            &game_recipe("[install]\ndir = \"x\"\nprefix = \"x-proton\"\n")
                .replace("slug = \"testgame\"", "slug = \"../escaped\""),
        );
        home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
        let paths = home.paths();
        installed_package(&home.games().join("x"), GAME);
        let launched = prepare_launch(&cli(), &paths, GAME, LaunchRequest::default())
            .and_then(|prepared| launch_prepared(&paths, prepared, false));
        assert!(launched.is_err());
        assert!(!home.root().join("escaped-launch.log").exists());
    }

    // -- the launch log and the end of a launch ------------------------------

    /// A title writing bytes that are not UTF-8, to a terminal that has gone
    /// away, still has every line in its log.
    #[test]
    fn the_log_keeps_everything_after_bad_bytes_and_a_closed_terminal() {
        let scratch = Scratch::new("tee");
        let log_path = scratch.root.join("launch.log");
        let log = Arc::new(Mutex::new(std::fs::File::create(&log_path).unwrap()));
        // A terminal nobody reads any more.
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { libc::close(fds[0]) };
        use std::os::fd::FromRawFd;
        let closed = unsafe { std::fs::File::from_raw_fd(fds[1]) };
        let mut input = b"first line\n\xff\xfe code page \x80\n".to_vec();
        input.extend(std::iter::repeat(b'x').take(200_000));
        input.extend_from_slice(b"\nlast line\n");
        tee(std::io::Cursor::new(input.clone()), log, closed)
            .join()
            .unwrap();
        assert_eq!(std::fs::read(&log_path).unwrap(), input);
    }

    struct FakeWine {
        scratch: Scratch,
        running: Arc<std::sync::atomic::AtomicBool>,
    }

    impl FakeWine {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            for tool in ["wine", "wineserver"] {
                let path = scratch.root.join(tool);
                std::fs::write(
                    &path,
                    format!(
                        "#!/bin/sh\necho \"{tool} $* $WINEPREFIX\" >> \"$(dirname \"$0\")/calls\"\n"
                    ),
                )
                .unwrap();
                make_executable(&path);
            }
            FakeWine {
                scratch,
                running: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            }
        }

        fn session(&self, grace: std::time::Duration) -> Arc<PrefixSession> {
            let running = Arc::clone(&self.running);
            Arc::new(PrefixSession {
                wine: self.scratch.root.join("wine"),
                wineserver: self.scratch.root.join("wineserver"),
                wine_prefix: self.scratch.root.join("prefix/pfx"),
                executable: "Game.exe".into(),
                grace,
                running: Box::new(move || running.load(std::sync::atomic::Ordering::SeqCst)),
            })
        }

        fn calls(&self) -> String {
            std::fs::read_to_string(self.scratch.root.join("calls")).unwrap_or_default()
        }

        fn detached(&self, grace: std::time::Duration) -> Forwarding {
            let forwarding = Forwarding {
                shared: Shared::new(),
                installed: false,
            };
            forwarding.set_session(Some(self.session(grace)));
            forwarding
        }
    }

    /// Wine starts a game in a session of its own, so a stopped launch left
    /// it running, without its service. Stopping now asks the game to close,
    /// waits, and then ends the prefix.
    #[test]
    fn a_stopped_launch_asks_the_game_to_close_and_then_ends_the_prefix() {
        let wine = FakeWine::new("stop-grace");
        let forwarding = wine.detached(std::time::Duration::from_millis(300));
        let started = std::time::Instant::now();
        forwarding.shared.deliver(libc::SIGTERM);
        forwarding.finish(std::time::Duration::from_secs(30));
        let calls = wine.calls();
        let close = calls.find("wine taskkill /IM Game.exe").expect(&calls);
        let kill = calls.find("wineserver -k").expect(&calls);
        assert!(close < kill, "{calls}");
        assert!(calls.contains("prefix/pfx"), "{calls}");
        assert!(started.elapsed() >= std::time::Duration::from_millis(300));
        assert_eq!(forwarding.received(), Some(libc::SIGTERM));
    }

    #[test]
    fn a_game_that_closes_when_asked_is_not_killed() {
        let wine = FakeWine::new("stop-closes");
        let forwarding = wine.detached(std::time::Duration::from_secs(30));
        let running = Arc::clone(&wine.running);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            running.store(false, std::sync::atomic::Ordering::SeqCst);
        });
        let started = std::time::Instant::now();
        forwarding.shared.deliver(libc::SIGINT);
        forwarding.finish(std::time::Duration::from_secs(30));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let calls = wine.calls();
        assert!(calls.contains("taskkill"), "{calls}");
        assert!(!calls.contains("wineserver -k"), "{calls}");
    }

    #[test]
    fn a_second_signal_ends_the_game_at_once() {
        let wine = FakeWine::new("stop-twice");
        let forwarding = wine.detached(std::time::Duration::from_secs(60));
        let started = std::time::Instant::now();
        forwarding.shared.deliver(libc::SIGTERM);
        let shared = Arc::clone(&forwarding.shared);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            shared.deliver(libc::SIGTERM);
        });
        forwarding.finish(std::time::Duration::from_secs(60));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(wine.calls().contains("wineserver -k"));
    }

    /// After the title exits, the launch waits a bounded time for the rest
    /// of its Wine session -- and a signal then is not swallowed.
    #[test]
    fn after_the_title_a_signal_still_ends_the_wait() {
        let wine = FakeWine::new("stop-after");
        let forwarding = wine.detached(std::time::Duration::from_millis(100));
        let started = std::time::Instant::now();
        forwarding.finish(std::time::Duration::from_millis(300));
        assert!(started.elapsed() >= std::time::Duration::from_millis(300));
        assert!(wine.calls().is_empty());

        let started = std::time::Instant::now();
        let shared = Arc::clone(&forwarding.shared);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            shared.deliver(libc::SIGINT);
        });
        forwarding.finish(std::time::Duration::from_secs(60));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(wine.calls().contains("wineserver -k"));

        // Nothing left running: nothing to wait for.
        wine.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let started = std::time::Instant::now();
        let quiet = wine.detached(std::time::Duration::from_secs(60));
        quiet.finish(std::time::Duration::from_secs(60));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    /// A signal before Wine is up, or while Proton is still making the
    /// prefix, starts nothing in the prefix: `wine taskkill` there would
    /// start a session of its own. A later signal, once Wine is up, still
    /// stops it.
    #[test]
    fn a_stop_with_no_wine_session_runs_nothing_in_the_prefix() {
        let wine = FakeWine::new("stop-nothing");
        wine.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let forwarding = wine.detached(std::time::Duration::from_millis(100));
        assert!(!forwarding
            .shared
            .stop(&wine.session(std::time::Duration::from_millis(100)), 0));
        forwarding.shared.deliver(libc::SIGINT);
        forwarding.finish(std::time::Duration::from_millis(100));
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(wine.calls().is_empty(), "{}", wine.calls());

        wine.running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        forwarding.shared.deliver(libc::SIGINT);
        forwarding.finish(std::time::Duration::from_secs(30));
        let calls = wine.calls();
        assert!(calls.contains("wine taskkill /IM Game.exe"), "{calls}");
        assert!(calls.contains("wineserver -k"), "{calls}");
    }

    /// A second signal kills at once, even while the request to close is
    /// itself still running, and the grace period starts with the first.
    #[test]
    fn a_second_signal_does_not_wait_for_the_request_to_close() {
        let wine = FakeWine::new("stop-slow-close");
        // A `wine taskkill` that takes its time, as it can while the game is
        // busy.
        let slow = wine.scratch.root.join("wine");
        std::fs::write(
            &slow,
            "#!/bin/sh\necho \"wine $* $WINEPREFIX\" >> \"$(dirname \"$0\")/calls\"\nexec sleep 30\n",
        )
        .unwrap();
        make_executable(&slow);
        let forwarding = wine.detached(std::time::Duration::from_secs(60));
        let started = std::time::Instant::now();
        forwarding.shared.deliver(libc::SIGTERM);
        let shared = Arc::clone(&forwarding.shared);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            shared.deliver(libc::SIGTERM);
        });
        forwarding.finish(std::time::Duration::from_secs(60));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(wine.calls().contains("wineserver -k"), "{}", wine.calls());

        // With no second signal, the kill comes at the end of the grace
        // period from the first, not after the request to close returns.
        let wine2 = FakeWine::new("stop-slow-grace");
        std::fs::copy(&slow, wine2.scratch.root.join("wine")).unwrap();
        let forwarding = wine2.detached(std::time::Duration::from_millis(500));
        let started = std::time::Instant::now();
        forwarding.shared.deliver(libc::SIGTERM);
        forwarding.finish(std::time::Duration::from_secs(60));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(wine2.calls().contains("wineserver -k"), "{}", wine2.calls());
    }

    /// The signal still reaches the title, before the session is ended.
    #[test]
    fn a_stopped_launch_still_signals_its_title() {
        let mut wine = FakeWine::new("stop-title");
        wine.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let forwarding = wine.detached(std::time::Duration::from_millis(100));
        let root = wine.scratch.root.clone();
        let title = wine.scratch.stand_in("title", &root, false);
        let shared = Arc::clone(&forwarding.shared);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            shared.deliver(libc::SIGTERM);
        });
        let status = forwarding.wait(&mut wine.scratch.children[title]).unwrap();
        assert_eq!(signal_of(&status), Some(libc::SIGTERM));
        forwarding.finish(std::time::Duration::from_secs(5));
        // No Wine session: nothing was started in the prefix.
        assert!(wine.calls().is_empty(), "{}", wine.calls());
    }

    // -- the runtime download -------------------------------------------------

    /// A tiny web server for the release feed and its files. Each path is
    /// answered with what `routes` says, every time it is asked.
    fn serve(routes: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>) -> String {
        use std::io::Read;
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in server.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => request.push(byte[0]),
                        _ => break,
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                let path = path.split('?').next().unwrap_or("/").to_string();
                let body = routes.lock().unwrap().get(&path).cloned();
                let _ = match body {
                    Some(body) => stream
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .and_then(|()| stream.write_all(&body)),
                    None => stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    ),
                };
            }
        });
        base
    }

    /// A runtime archive as runtime.yml makes one.
    fn runtime_archive(root: &Path, version: &str) -> Vec<u8> {
        let tree = root.join("archive-tree");
        let top = tree.join(format!("ferestre-runtime-{version}"));
        std::fs::create_dir_all(&top).unwrap();
        std::fs::write(top.join("proton"), "#!/bin/sh\n").unwrap();
        std::fs::write(top.join("version"), format!("1 ferestre-{version}\n")).unwrap();
        let archive = root.join("archive.tar.xz");
        let status = Command::new("tar")
            .arg("-cJf")
            .arg(&archive)
            .arg("-C")
            .arg(&tree)
            .arg(format!("ferestre-runtime-{version}"))
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(archive).unwrap()
    }

    /// Everything `install-runtime` fetches checks out against the release
    /// key before it is unpacked; anything that does not is thrown away.
    #[test]
    fn install_runtime_installs_only_what_the_release_key_signed() {
        use ferestre_core::signing::testing::TestKey;
        let home = Home::new("install-runtime");
        let paths = home.paths();
        let version = "11.0.3";
        let name = format!("ferestre-runtime-{version}.tar.xz");
        let archive = runtime_archive(home.root(), version);
        let key = TestKey::new();
        let routes = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let base = serve(Arc::clone(&routes));
        let unsigned = json!([
            { "tag_name": "v0.1.13", "assets": [] },
            { "tag_name": format!("runtime-{version}"), "assets": [
                { "name": name, "browser_download_url": format!("{base}/download/{name}") },
            ]},
        ]);
        let feed = json!([
            { "tag_name": "v0.1.13", "assets": [] },
            { "tag_name": format!("runtime-{version}"), "assets": [
                { "name": name, "browser_download_url": format!("{base}/download/{name}") },
                {
                    "name": format!("{name}.minisig"),
                    "browser_download_url": format!("{base}/download/{name}.minisig"),
                },
            ]},
        ]);
        {
            let mut routes = routes.lock().unwrap();
            routes.insert("/releases".to_string(), unsigned.to_string().into_bytes());
            routes.insert(format!("/download/{name}"), archive.clone());
            // Signed, but by somebody else.
            let stranger = TestKey::new();
            routes.insert(
                format!("/download/{name}.minisig"),
                stranger
                    .sign_with_comment(&format!("file:{name}"), &archive)
                    .into_bytes(),
            );
        }
        let api = format!("{base}/releases");
        let run = |cli: &Cli| {
            install_runtime(
                cli,
                &paths,
                &api,
                &key.public(),
                &cancel::Cancellation::default(),
            )
        };
        let runtimes = paths.runtimes_dir();
        let cached = paths.cache_dir().join("runtime").join(&name);
        // Left by an install that was killed.
        let abandoned = runtimes.join(format!(".{version}.installing-4000000000"));
        std::fs::create_dir_all(&abandoned).unwrap();

        // A release that publishes no signature is refused before anything is
        // downloaded.
        let error = run(&cli()).unwrap_err();
        assert!(
            format!("{error:#}").contains("publishes no signature"),
            "{error:#}"
        );
        assert!(!cached.exists());

        routes
            .lock()
            .unwrap()
            .insert("/releases".to_string(), feed.to_string().into_bytes());
        let error = format!("{:#}", run(&cli()).unwrap_err());
        assert!(error.contains("nothing was installed"), "{error}");
        assert!(
            error.contains("not signed by the Ferestre release key"),
            "{error}"
        );
        assert!(!cached.exists(), "a bad download stays in the cache");
        assert!(!runtimes.join(version).exists());

        routes.lock().unwrap().insert(
            format!("/download/{name}.minisig"),
            key.sign_with_comment(&format!("timestamp:1 file:{name}"), &archive)
                .into_bytes(),
        );
        // A corrupt archive already in the cache is replaced, not reused.
        std::fs::write(&cached, b"corrupt").unwrap();
        assert_eq!(code(run(&cli()).unwrap()), code(ExitCode::SUCCESS));
        assert!(InstalledRuntime::at(&runtimes.join(version)).is_ok());
        assert!(!cached.exists(), "the archive is not kept once installed");
        assert!(!abandoned.exists());
        let leftovers: Vec<_> = std::fs::read_dir(&runtimes)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // Installed already: said so, and nothing fetched.
        routes.lock().unwrap().remove(&format!("/download/{name}"));
        assert_eq!(code(run(&cli()).unwrap()), code(ExitCode::SUCCESS));
    }

    /// Launcher releases are many and runtime releases few, so the newest
    /// runtime can be past the first page of the feed.
    #[test]
    fn the_newest_runtime_is_found_past_the_first_page() {
        let launcher_page = |n: usize| {
            let releases: Vec<Value> = (0..n)
                .map(|i| json!({ "tag_name": format!("v0.{i}"), "assets": [] }))
                .collect();
            serde_json::to_string(&releases).unwrap()
        };
        let runtime_page = json!([{ "tag_name": "runtime-11.0.1", "assets": [
            { "name": "ferestre-runtime-11.0.1.tar.xz", "browser_download_url": "https://x/r" }
        ]}])
        .to_string();
        let asked = std::cell::Cell::new(0);
        let release = newest_runtime_release(|page| {
            asked.set(page);
            Ok(if page < 3 {
                launcher_page(RELEASES_PER_PAGE)
            } else {
                runtime_page.clone()
            })
        })
        .unwrap();
        assert_eq!(release.tag, "runtime-11.0.1");
        assert_eq!(asked.get(), 3);
        // A short page is the last one.
        let error = newest_runtime_release(|_| Ok(launcher_page(3))).unwrap_err();
        assert!(error.to_string().contains("no runtime"), "{error}");
    }

    // -- updates and doctor -------------------------------------------------

    /// "Cannot tell" is not "up to date", and one bad answer does not end the
    /// check for every other title.
    #[test]
    fn updates_say_unknown_and_carry_on_past_a_bad_answer() {
        let home = Home::new("updates");
        let paths = home.paths();
        for (id, content) in [("9ZZBADANSWER", "bad"), ("9ZZGOODANSWR", "good")] {
            let dir = home.games().join(id);
            installed_package(&dir, id);
            install::save(
                paths.state_dir(),
                &install::Record {
                    product_id: id.into(),
                    dir,
                    content_ids: vec![content.into()],
                    installed_at: None,
                    package_version: Some("1.0.0.0".into()),
                },
            )
            .unwrap();
        }
        home.client(
            "case \"$2\" in bad) echo 'not json';; good) echo '{\"version\":\"2.0.0.0\"}';; esac",
        );
        let results = check_updates(&paths, &home.root().join("client/xodus-cli")).unwrap();
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(results[0]["product_id"], "9ZZGOODANSWR");
        assert_eq!(update_state(&results[0]), "update available");
        assert_eq!(
            update_state(&json!({ "update_available": null })),
            "unknown (no version to compare)"
        );
        assert_eq!(
            update_state(&json!({ "update_available": false })),
            "up to date"
        );
    }

    /// Doctor sees a runtime `install-runtime` downloaded, and a recipe of the
    /// person's own that does not parse.
    #[test]
    fn doctor_sees_managed_runtimes_and_broken_user_recipes() {
        let home = Home::new("doctor");
        home.runtime("ferestre-11.0.2", Some(&["loader.memfd-main-image"]));
        home.recipe("config/ferestre/titles", "broken.toml", "schema = [");
        let checks = doctor_checks(&home.paths());
        let runtime = checks.iter().find(|c| c.section == "runtime").unwrap();
        assert_eq!(runtime.status, CheckStatus::Ok, "{runtime:?}");
        assert!(runtime
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("ferestre-11.0.2"));
        let recipes = checks.iter().find(|c| c.section == "recipes").unwrap();
        assert_eq!(recipes.status, CheckStatus::Fail, "{recipes:?}");
        assert!(recipes
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("broken.toml"));
    }
}
