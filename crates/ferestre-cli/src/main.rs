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

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use ferestre_core::catalog;
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
        /// that wants the overlay, controller config and Remote Play.
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
        Cmd::Env => cmd_env(cli),
        Cmd::Version => cmd_version(cli),
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

fn cmd_version(cli: &Cli) -> Result<ExitCode> {
    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "name": "ferestre",
            "version": env!("CARGO_PKG_VERSION"),
        }));
    } else {
        println!("{}", env!("CARGO_PKG_VERSION"));
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
        let output = update_command(&xodus_cli, content_id)
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

    // Checked before anything is started, because the alternative is a title
    // that exits 1 with an empty log and someone guessing which of eleven
    // patches is missing.
    let runtime = InstalledRuntime::discover(&paths)?;
    let registry = registry_for(&paths);
    let assessment = runtime::assess(recipe, &runtime, registry.as_ref());
    let verdict = gate(&assessment);
    if verdict == Gate::Refused && !force {
        bail!("{}", refusal(&assessment, &runtime));
    }
    for line in warnings(recipe, &assessment, verdict, force) {
        eprintln!("{line}");
    }

    // Before anything is started: a runtime class the prefix has never been told
    // about cannot be activated, however complete the implementation. Cheap when
    // there is nothing to do -- reading one file -- and it is the difference
    // between a patched runtime working and merely existing.
    if let Some(registry) = &registry {
        register_winrt_classes(&paths, recipe, registry, &runtime);
    }

    let dirs = launch_dirs(&paths)?;
    let options = launch::Options {
        steam_integration: steam,
        xuid: paths.var("XGR_XUID").map(str::to_string),
        game_dir: Some(paths.install_dir(recipe)?),
        prefix_dir: Some(paths.prefix_dir(recipe)?),
    };
    let plan = launch::plan(recipe, &dirs, &options);

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
    start_service(&dirs)?;

    run_teed(plan.to_command(), &launch::log_path(recipe, &dirs))
}

fn cmd_install(cli: &Cli, product_id: &str, dir: Option<&Path>) -> Result<ExitCode> {
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

    let command = streaming_command(&xodus_cli, &product_id, &dest);

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
    let code = run_inherited(command)?;

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
fn register_winrt_classes(
    paths: &Paths,
    recipe: &Recipe,
    registry: &Registry,
    runtime: &InstalledRuntime,
) {
    let classes = registry.winrt_classes();
    if classes.is_empty() {
        return;
    }
    let Ok(prefix) = paths.prefix_dir(recipe) else {
        return;
    };
    // A prefix that does not exist yet is about to be created from the default
    // one, and that copy would overwrite anything written now.
    if !winrt::registry_path(&prefix).is_file() {
        return;
    }
    let missing = winrt::missing_in_prefix(&prefix, &classes);
    if missing.is_empty() {
        return;
    }

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
            .env("WINEPREFIX", prefix.join("pfx"))
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
    let path = recipe.save_to(&paths.user_titles_dir())?;
    Ok(Some(path))
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

fn cmd_install_runtime(cli: &Cli) -> Result<ExitCode> {
    let paths = Paths::from_env()?;
    let scripts = paths
        .scripts_dir()
        .ok_or_else(|| anyhow!("cannot find the scripts directory; set XODUS_REPO_DIR"))?;
    let script = scripts.join("install-xodus-proton.sh");
    if !script.is_file() {
        bail!("{} is missing", script.display());
    }
    if !paths.build_dir().is_dir() {
        // It builds inside a container from a Proton tree no package can carry,
        // so say where that has to be rather than failing deep in the script.
        bail!(
            "no Proton build tree at {} -- the runtime is built from source, not shipped: \
             see docs/RECIPES.md section 1, then set XODUS_BUILD_DIR",
            paths.build_dir().display()
        );
    }

    if cli.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "plan": { "program": script.display().to_string(), "args": [] },
        }));
        return Ok(ExitCode::SUCCESS);
    }
    Ok(ExitCode::from(run_inherited(Command::new(script))?))
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
    Ok(launch::Dirs {
        games_dir: paths.games_dir().to_path_buf(),
        cli_dir: paths
            .cli_dir()
            .ok_or_else(|| anyhow!("no xodus-cli found; set XODUS_CLI_DIR (see: ferestre doctor)"))?
            .to_path_buf(),
        proton_dir: paths
            .runtime_dir()
            .ok_or_else(|| anyhow!("no patched Proton found; run: ferestre install-runtime"))?
            .to_path_buf(),
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
fn run_teed(mut command: Command, log: &Path) -> Result<ExitCode> {
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

    let status = child.wait().context("waiting for the title")?;
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

/// Start `xodus-service` if it is not already up.
///
/// The GDK runtime's Xbox-side calls (sign-in, tokens) go through it, and a
/// title sits on its sign-in screen without one. A stale socket is removed
/// first: the service does not clean up after a crash, and a leftover socket
/// makes the runtime think there is a service to talk to.
fn start_service(dirs: &launch::Dirs) -> Result<()> {
    if process_running("xodus-service") {
        return Ok(());
    }
    let binary = dirs.xodus_service();
    if !binary.is_file() {
        bail!(
            "{} is missing; the runtime's Xbox-side calls go through it",
            binary.display()
        );
    }
    if let Some(socket) = runtime_socket() {
        let _ = std::fs::remove_file(socket);
    }
    let log = std::env::temp_dir().join("xodus-service.log");
    let out = std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
    let errs = out
        .try_clone()
        .context("duplicating the service log handle")?;
    Command::new(&binary)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(errs))
        .spawn()
        .with_context(|| format!("starting {}", binary.display()))?;
    eprintln!(":: started xodus-service (log: {})", log.display());
    // It has to be listening before the title asks it anything, and it offers
    // nothing to wait on. Same two seconds launch-gdk.sh sleeps.
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(())
}

fn runtime_socket() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("xodus.sock"))
}

/// Whether a process with this name is running, read from `/proc`.
///
/// `pgrep -x` is what the shell script uses; reading `/proc` avoids depending
/// on procps being installed, which it is not in every container.
fn process_running(name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            // `Option::is_none_or` would say this better and is newer than the
            // MSRV this workspace promises.
            .map_or(true, |n| !n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(path.join("comm")) {
            // /proc/PID/comm is truncated to 15 characters, so a long name is
            // compared on the prefix the kernel kept.
            let comm = comm.trim_end();
            if comm == name || (comm.len() == 15 && name.starts_with(comm)) {
                return true;
            }
        }
    }
    false
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
