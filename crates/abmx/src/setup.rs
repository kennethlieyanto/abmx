//! `abmx setup`: an attended quickstart.
//!
//! Every step is idempotent. Rerunning the command skips what is already done,
//! and answers are saved as they are given, so Ctrl-C never loses earlier work.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use abmx_core::SinkAccount;
use anyhow::{bail, Context, Result};
use cliclack::{confirm, input, intro, log, note, outro, outro_cancel, password, select, spinner};

use crate::config::{Config, INIT_TEMPLATE};
use crate::config_edit::{write_secret_file, ConfigDocument};
use crate::credentials;
use crate::gmail_auth;
use crate::labels::{self, DiscoveredLabel};
use crate::paths;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub config: Option<PathBuf>,
    pub no_browser: bool,
    pub manual: bool,
    /// Re-offer labels the user previously declined.
    pub review: bool,
    /// Re-ask account mappings that already exist.
    pub remap: bool,
}

const GMAIL_CLIENT_FILE: &str = "gmail-client.json";
const ACTUAL_API_KEY_FILE: &str = "actual-api-key";
const ACTUAL_SERVER_PASSWORD_FILE: &str = "actual-server-password";
const ACTUAL_BUDGET_PASSWORD_FILE: &str = "actual-budget-password";

pub fn run(options: &Options) -> Result<i32> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("`abmx setup` is interactive; run it in a terminal");
    }

    match setup(options) {
        Ok(()) => Ok(0),
        Err(error) if interrupted(&error) => {
            let _ = outro_cancel("setup cancelled; completed steps were saved");
            Ok(0)
        }
        Err(error) => Err(error),
    }
}

fn setup(options: &Options) -> Result<()> {
    intro("abmx setup")?;

    let path = resolve_config_path(options.config.as_deref());
    let existed = path.exists();
    let mut doc = ConfigDocument::open_or_template(&path)?;

    // 1. Config ------------------------------------------------------------
    log::step("Configuration")?;
    if existed {
        log::success(format!("using {}", path.display()))?;
    } else {
        let create = confirm(format!("Write a starter config to {}?", path.display()))
            .initial_value(true)
            .interact()?;
        if !create {
            outro_cancel("no configuration written")?;
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, INIT_TEMPLATE)
            .with_context(|| format!("writing {}", path.display()))?;
        log::success(format!("wrote {}", path.display()))?;
    }

    // 2. Discovery rule ----------------------------------------------------
    step_discovery(&path, &mut doc)?;

    // 3. History window ----------------------------------------------------
    step_history(&path, &mut doc)?;

    // 4. Gmail -------------------------------------------------------------
    let gmail_ready = step_gmail(&path, &mut doc, options)?;

    // 5. Labels ------------------------------------------------------------
    let discovered = step_labels(&path, &mut doc, options, gmail_ready)?;

    // 6. Accounts ----------------------------------------------------------
    step_accounts(&path, &mut doc, options, discovered.as_deref())?;

    // Done -----------------------------------------------------------------
    let (config, _) = reload(&path)?;
    config.validate_labels()?;
    summary(&path, &config)?;
    outro("Setup finished; rerun `abmx setup` whenever something changes")?;
    Ok(())
}

fn reload(path: &Path) -> Result<(Config, PathBuf)> {
    let config = Config::load(path)?;
    Ok((config, Config::base_dir(path)))
}

fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(path) = std::env::var_os("ABMX_CONFIG") {
        return PathBuf::from(path);
    }

    let xdg = paths::xdg_config_dir().join("abmx/config.toml");
    if xdg.exists() {
        return xdg;
    }
    let cwd = PathBuf::from("abmx.toml");
    if cwd.exists() {
        return cwd;
    }
    xdg
}

fn interrupted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
}

// --------------------------------------------------------- Discovery rule

fn step_discovery(path: &Path, doc: &mut ConfigDocument) -> Result<()> {
    log::step("Discovery rule")?;
    let (config, _) = reload(path)?;
    let current = config.labels.prefix.clone();

    log::info("labels starting with this prefix are discovered; the rest becomes the parser id")?;

    let prefix: String = input("Gmail label prefix")
        .placeholder("receipt/")
        .default_input(&current)
        .validate(|value: &String| {
            if value.trim().is_empty() {
                Err("the prefix is required")
            } else {
                Ok(())
            }
        })
        .interact()?;
    let prefix = prefix.trim().to_owned();

    log::info(format!("using `{prefix}`"))?;

    if prefix != current {
        doc.set_string(&["labels", "prefix"], &prefix)?;
        doc.save()?;
        log::success(format!("prefix set to `{prefix}`"))?;
    }

    Ok(())
}

// ---------------------------------------------------------- History window

fn step_history(path: &Path, doc: &mut ConfigDocument) -> Result<()> {
    log::step("History window")?;
    let (config, _) = reload(path)?;
    let current = config.labels.since;
    let today = chrono::Local::now().date_naive();
    let days_ago = |days: i64| (today - chrono::Duration::days(days)).to_string();

    log::info(
        "only mail on/after this date is imported; `abmx samples` still takes the \
         newest messages for authoring",
    )?;

    let mut prompt = select("History window");
    if let Some(date) = current {
        prompt = prompt.item(
            "keep".to_owned(),
            format!("Keep current ({date})"),
            "no change",
        );
    }
    prompt = prompt
        .item(
            "7".to_owned(),
            "Last 7 days",
            format!("since {}", days_ago(7)),
        )
        .item(
            "30".to_owned(),
            "Last 30 days",
            format!("since {}", days_ago(30)),
        )
        .item(
            "90".to_owned(),
            "Last 90 days",
            format!("since {}", days_ago(90)),
        )
        .item(
            "all".to_owned(),
            "All history",
            "not recommended: scans every labelled message",
        )
        .item("custom".to_owned(), "Custom date", "YYYY-MM-DD");

    prompt = prompt.initial_value(if current.is_some() {
        "keep".to_owned()
    } else {
        "30".to_owned()
    });

    let choice: String = prompt.interact()?;
    let selected = match choice.as_str() {
        "keep" => current,
        "7" => Some(today - chrono::Duration::days(7)),
        "30" => Some(today - chrono::Duration::days(30)),
        "90" => Some(today - chrono::Duration::days(90)),
        "all" => None,
        "custom" => {
            let value: String = input("Since (YYYY-MM-DD)")
                .placeholder("2026-11-01")
                .validate(|value: &String| {
                    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
                        .map(|_| ())
                        .map_err(|_| "expected a date like 2026-11-01".to_owned())
                })
                .interact()?;
            chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
        }
        _ => unreachable!("the select only returns declared choices"),
    };

    if selected == current {
        return Ok(());
    }

    match selected {
        Some(date) => {
            doc.set_string(&["labels", "since"], &date.to_string())?;
            doc.save()?;
            log::success(format!("fetching mail since {date}"))?;
        }
        None => {
            doc.remove(&["labels", "since"])?;
            doc.save()?;
            log::warning("history window cleared; every labelled message will be fetched")?;
        }
    }

    Ok(())
}

// ------------------------------------------------------------------ Gmail

fn step_gmail(path: &Path, doc: &mut ConfigDocument, options: &Options) -> Result<bool> {
    log::step("Gmail access")?;
    let (config, base) = reload(path)?;

    let token_path = config.gmail_token_path(&base);
    if token_path.exists() {
        let spinner = spinner();
        spinner.start("Checking the stored Gmail session");
        match config.build_gmail(&base) {
            Ok(fetcher) => match fetcher.client().profile() {
                Ok(profile) => {
                    spinner.stop(format!("Gmail: {}", profile.email_address));
                    return Ok(true);
                }
                Err(error) => {
                    spinner.error(format!("stored Gmail session is not usable: {error}"));
                    log::info("run `abmx auth gmail` to authorize again")?;
                }
            },
            Err(error) => {
                spinner.error(format!("stored Gmail session is not usable: {error}"));
            }
        }
    }

    let credentials_ready = config.gmail_credentials(&base).is_ok();
    if !credentials_ready {
        note(
            "Google Cloud setup",
            "abmx needs an OAuth client for a *Desktop app*:\n\n\
             1. Create (or pick) a Google Cloud project.\n\
             2. Enable the Gmail API.\n\
             3. OAuth consent screen: External, then add yourself as a test user.\n\
             4. Credentials -> Create credentials -> OAuth client ID -> Desktop app.\n\
             5. Download the JSON file.\n\n\
             Details: docs/gmail.md",
        )?;

        let configure = confirm("Point abmx at the downloaded OAuth client JSON now?")
            .initial_value(true)
            .interact()?;
        if !configure {
            log::info("skipped Gmail setup; rerun `abmx setup` when the JSON is ready")?;
            return Ok(false);
        }

        let entered: String = input("Path to the OAuth client JSON")
            .placeholder("~/Downloads/client_secret_....apps.googleusercontent.com.json")
            .validate(|value: &String| {
                let path = resolve_user_path(value);
                if !path.exists() {
                    return Err(format!("{} does not exist", path.display()));
                }
                credentials::load_google_client(&path)
                    .map(|_| ())
                    .map_err(|error| format!("{error:#}"))
            })
            .interact()?;

        let source = resolve_user_path(&entered);
        let destination = base.join(GMAIL_CLIENT_FILE);
        if source != destination {
            let bytes =
                std::fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
            write_private_bytes(&destination, &bytes)?;
        }
        doc.set_gmail_credentials_file(GMAIL_CLIENT_FILE)?;
        // `credentials_file` is authoritative for the client id; a stale or
        // placeholder `client_id` left in the file would silently win.
        doc.remove(&["fetch", "gmail", "client_id"])?;
        doc.save()?;
        log::success(format!("OAuth client saved to {}", destination.display()))?;
    }

    let (config, base) = reload(path)?;
    let authorize = confirm("Authorize Gmail now? A browser window will open.")
        .initial_value(true)
        .interact()?;
    if !authorize {
        log::info("skipped Gmail authorization; rerun `abmx setup` when ready")?;
        return Ok(false);
    }

    match gmail_auth::authorize_gmail(
        &config,
        &base,
        &gmail_auth::Options {
            no_browser: options.no_browser,
            port: None,
            manual: options.manual,
        },
    ) {
        Ok(token) => {
            log::success(format!("authorized; token at {}", token.display()))?;
            Ok(true)
        }
        Err(error) => {
            log::error(format!("{error:#}"))?;
            log::info("run `abmx auth gmail` when ready; the rest of setup can continue")?;
            Ok(false)
        }
    }
}

// ----------------------------------------------------------------- Labels

/// Returns the discovered label names when the label step actually ran.
fn step_labels(
    path: &Path,
    doc: &mut ConfigDocument,
    options: &Options,
    gmail_ready: bool,
) -> Result<Option<Vec<String>>> {
    log::step("Labels")?;
    if !gmail_ready {
        log::info("skipped: Gmail is not authorized")?;
        return Ok(None);
    }

    let (config, base) = reload(path)?;
    let fetcher = match config.build_gmail(&base) {
        Ok(fetcher) => fetcher,
        Err(error) => {
            log::error(format!("{error:#}"))?;
            return Ok(None);
        }
    };

    let spinner = spinner();
    spinner.start("Looking for routed Gmail labels");
    let found = match fetcher.routed_labels() {
        Ok(found) => found,
        Err(error) => {
            spinner.error(format!("could not list Gmail labels: {error}"));
            return Ok(None);
        }
    };
    let discovered: Vec<String> = found.into_iter().map(|label| label.name).collect();
    spinner.stop(format!(
        "{} label(s) under `{}`",
        discovered.len(),
        config.labels.prefix
    ));

    if discovered.is_empty() {
        let plan = labels::plan(&discovered, &config.labels);
        for parser_id in &plan.orphans {
            log::warning(format!(
                "`{}{parser_id}` is mapped in [labels.accounts] but no such Gmail label exists",
                config.labels.prefix
            ))?;
        }
        note(
            "No routed labels yet",
            format!(
                "abmx only imports mail that gmailctl routes to it.\n\n\
                 Create a label and a filter under `{}`, for example:\n\n  \
                 {}example.card_debit\n\n\
                 Labels outside `{}` are ignored.\n\n\
                 gmailctl owns the classification. Add a rule under\n  \
                 ~/.gmailctl/config.jsonnet\n\
                 then run `gmailctl apply` and `abmx setup` again.\n\n\
                 See docs/gmail.md.",
                config.labels.prefix, config.labels.prefix, config.labels.prefix
            ),
        )?;
        return Ok(Some(discovered));
    }

    let plan = labels::plan(&discovered, &config.labels);
    for label in &plan.invalid {
        log::warning(format!(
            "`{label}`: the remainder after `{}` is not a parser id",
            config.labels.prefix
        ))?;
    }
    for parser_id in &plan.orphans {
        log::warning(format!(
            "`{}{parser_id}` is mapped in [labels.accounts] but no such Gmail label exists",
            config.labels.prefix
        ))?;
    }
    if !plan.managed.is_empty() {
        log::info(format!(
            "{} label(s) are already managed",
            plan.managed.len()
        ))?;
    }

    let mut candidates: Vec<DiscoveredLabel> = plan.unmanaged.clone();
    if options.review {
        candidates.extend(plan.ignored.iter().cloned());
    }

    if candidates.is_empty() {
        log::success("nothing new to decide")?;
        return Ok(Some(discovered));
    }

    for label in &candidates {
        let import = confirm(format!("Import transactions labelled `{}`?", label.label))
            .initial_value(true)
            .interact()?;

        if import {
            doc.remove_ignore(&label.parser_id)?;
        } else {
            doc.add_ignore(&label.parser_id)?;
        }
        // Save per answer so a Ctrl-C later does not lose decisions.
        doc.save()?;
    }

    Ok(Some(discovered))
}

// --------------------------------------------------------------- Accounts

fn step_accounts(
    path: &Path,
    doc: &mut ConfigDocument,
    options: &Options,
    discovered: Option<&[String]>,
) -> Result<()> {
    log::step("Ledger accounts")?;

    let Some(discovered) = discovered else {
        log::info("skipped: no label decisions yet")?;
        return Ok(());
    };

    if discovered.is_empty() {
        log::info("skipped: no routed labels")?;
        return Ok(());
    }

    // Do not demand a ledger when there is nothing to map.
    let (current, _) = reload(path)?;
    let initial = labels::plan(discovered, &current.labels);
    let nothing_to_do =
        initial.unmanaged.is_empty() && (!options.remap || initial.managed.is_empty());
    if nothing_to_do {
        log::success("nothing to map")?;
        return Ok(());
    }

    let (mut config, mut base) = reload(path)?;

    let sink = match config.build_sink(&base) {
        Ok(sink) => sink,
        Err(error) => {
            log::warning(format!("the ledger is not configured: {error}"))?;
            note(
                "Actual Budget",
                "abmx drives Actual's official Node client through a bundled helper.\n\n\
                 Have your Actual server URL, a budget-scoped API key (Settings →\n\
                 General → Security → API keys), and the budget's sync id ready.\n\
                 Details: docs/actual.md.",
            )?;

            let configure = confirm("Configure Actual now?")
                .initial_value(true)
                .interact()?;
            if !configure {
                log::info("skipped account mapping; rerun `abmx setup` when the ledger is ready")?;
                return Ok(());
            }

            configure_actual(path, doc, &config, &base)?;
            let (next, next_base) = reload(path)?;
            config = next;
            base = next_base;

            match config.build_sink(&base) {
                Ok(value) => value,
                Err(error) => {
                    log::error(format!("still could not build the sink: {error:#}"))?;
                    return Ok(());
                }
            }
        }
    };

    let spinner = spinner();
    spinner.start("Reading accounts from the actual helper");
    let accounts = match sink.accounts() {
        Ok(accounts) => {
            spinner.stop(format!("{} account(s)", accounts.len()));
            accounts
        }
        Err(error) => {
            spinner.error(format!("could not read accounts: {error}"));
            log::info(
                "is the Actual server reachable and the API key valid? \
                 Run `abmx status` for details, then rerun `abmx setup`.",
            )?;
            return Ok(());
        }
    };

    if accounts.is_empty() {
        log::warning(format!(
            "{} exposes no accounts; create one in the ledger first",
            sink.name()
        ))?;
        return Ok(());
    }

    let open: Vec<&SinkAccount> = accounts.iter().filter(|account| !account.closed).collect();
    if open.is_empty() {
        log::warning("every ledger account is closed; nothing to map")?;
        return Ok(());
    }

    let plan = labels::plan(discovered, &config.labels);
    let pending: Vec<&DiscoveredLabel> = plan.unmanaged.iter().collect();

    if pending.is_empty() && !options.remap {
        log::success("nothing to map")?;
        return Ok(());
    }

    for label in &pending {
        let mut prompt = select(format!("Account for `{}`", label.label)).filter_mode();
        for account in &open {
            let hint = if account.off_budget {
                format!("{} · off-budget", account.id)
            } else {
                account.id.clone()
            };
            prompt = prompt.item(account.id.clone(), account.name.clone(), hint);
        }
        prompt = prompt.item(String::new(), "Skip for now", "");

        let chosen: String = prompt.interact()?;
        if chosen.is_empty() {
            log::info(format!("skipped `{}`", label.label))?;
            continue;
        }

        doc.set_account(&label.parser_id, &chosen)?;
        doc.remove_ignore(&label.parser_id)?;
        doc.save()?;
        log::success(format!("{} -> {chosen}", label.label))?;
    }

    if options.remap {
        for label in &plan.managed {
            let mut prompt = select(format!(
                "Account for `{}` (currently `{}`)",
                label.label, label.account
            ))
            .filter_mode()
            .initial_value(label.account.clone());
            for account in &open {
                let hint = if account.off_budget {
                    format!("{} · off-budget", account.id)
                } else {
                    account.id.clone()
                };
                prompt = prompt.item(account.id.clone(), account.name.clone(), hint);
            }
            prompt = prompt.item(String::new(), "Keep the current mapping", "");

            let chosen: String = prompt.interact()?;
            if chosen.is_empty() || chosen == label.account {
                continue;
            }
            doc.set_account(&label.parser_id, &chosen)?;
            doc.save()?;
            log::success(format!("{} -> {chosen}", label.label))?;
        }
    }

    Ok(())
}

fn configure_actual(
    path: &Path,
    doc: &mut ConfigDocument,
    config: &Config,
    base: &Path,
) -> Result<()> {
    let existing = config.sink.actual.as_ref();

    let server_url: String = input("Actual server URL")
        .placeholder("http://localhost:5006")
        .default_input(
            existing
                .and_then(|actual| actual.server_url.as_deref())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("http://localhost:5006"),
        )
        .validate(|value: &String| {
            if value.trim().is_empty() {
                Err("the Actual server URL is required")
            } else if !value.trim().starts_with("http://") && !value.trim().starts_with("https://")
            {
                Err("expected an http:// or https:// URL")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let sync_id: String = input("Actual budget sync id")
        .placeholder("00000000-0000-0000-0000-000000000000")
        .validate(|value: &String| {
            if value.trim().is_empty() {
                Err("the sync id is required")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let env_key = std::env::var("ABMX_ACTUAL_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let has_key = env_key.is_some()
        || existing
            .and_then(|actual| actual.api_key.as_deref())
            .is_some_and(|value| !value.trim().is_empty())
        || existing
            .and_then(|actual| actual.api_key_file.as_ref())
            .is_some_and(|path| paths::resolve_path(base, path).exists());

    let env_password = std::env::var("ABMX_ACTUAL_SERVER_PASSWORD")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let has_server_password = env_password.is_some()
        || existing
            .and_then(|actual| actual.server_password.as_deref())
            .is_some_and(|value| !value.trim().is_empty())
        || existing
            .and_then(|actual| actual.server_password_file.as_ref())
            .is_some_and(|path| paths::resolve_path(base, path).exists());

    let mut api_key_file = None;
    let mut server_password_file = None;
    if !has_key && !has_server_password {
        let key: String =
            password("Actual API key (if available; blank to use the server password)")
                .allow_empty()
                .interact()?;

        if !key.trim().is_empty() {
            let file = base.join(ACTUAL_API_KEY_FILE);
            write_secret_file(&file, &key)?;
            api_key_file = Some(ACTUAL_API_KEY_FILE);
            log::success(format!("API key saved to {}", file.display()))?;
        } else {
            let value: String = password("Actual server password")
                .validate(|value: &String| {
                    if value.trim().is_empty() {
                        Err("an API key or the server password is required")
                    } else {
                        Ok(())
                    }
                })
                .interact()?;
            let file = base.join(ACTUAL_SERVER_PASSWORD_FILE);
            write_secret_file(&file, &value)?;
            server_password_file = Some(ACTUAL_SERVER_PASSWORD_FILE);
            log::success(format!("server password saved to {}", file.display()))?;
        }
    }

    let has_budget_password = std::env::var("ABMX_ACTUAL_BUDGET_PASSWORD")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
        || existing
            .and_then(|actual| actual.budget_password.as_deref())
            .is_some_and(|value| !value.trim().is_empty())
        || existing
            .and_then(|actual| actual.budget_password_file.as_ref())
            .is_some_and(|path| paths::resolve_path(base, path).exists());

    let mut budget_password_file = None;
    if !has_budget_password {
        let value: String = password("Actual budget password (blank if unencrypted)")
            .allow_empty()
            .interact()?;
        if !value.trim().is_empty() {
            let file = base.join(ACTUAL_BUDGET_PASSWORD_FILE);
            write_secret_file(&file, &value)?;
            budget_password_file = Some(ACTUAL_BUDGET_PASSWORD_FILE);
            log::success(format!("budget password saved to {}", file.display()))?;
        }
    }

    doc.set_actual(
        server_url.trim(),
        sync_id.trim(),
        api_key_file,
        server_password_file,
        budget_password_file,
    )?;
    doc.save()?;
    log::success(format!("updated {}", path.display()))?;
    Ok(())
}

// ----------------------------------------------------------------- Output

fn summary(path: &Path, config: &Config) -> Result<()> {
    let mut lines = Vec::new();
    lines.push(format!("config: {}", path.display()));

    if config.fetch.gmail.is_some() {
        let token = config.gmail_token_path(&Config::base_dir(path));
        lines.push(format!(
            "gmail:  {}",
            if token.exists() {
                "authorized"
            } else {
                "not authorized (rerun `abmx setup`)"
            }
        ));
    } else {
        lines.push("gmail:  not configured (`abmx setup`)".to_owned());
    }

    match config.build_sink(&Config::base_dir(path)) {
        Ok(sink) => {
            let target = config
                .sink
                .actual
                .as_ref()
                .and_then(|actual| actual.server_url.as_deref())
                .map(|url| format!(" ({url})"))
                .unwrap_or_default();
            lines.push(format!("sink:   {}{target}", sink.name()));
        }
        Err(_) => lines.push("sink:   not configured (`abmx setup`)".to_owned()),
    }

    lines.push(format!(
        "labels: {} managed, {} ignored (prefix `{}`)",
        config.labels.accounts.len(),
        config.labels.ignore.len(),
        config.labels.prefix
    ));
    lines.push(format!(
        "window: {}",
        config
            .labels
            .since
            .map(|date| format!("since {date}"))
            .unwrap_or_else(|| "all history".to_owned())
    ));
    if config.labels.accounts.is_empty() {
        lines.push("        nothing is imported until a label is mapped".to_owned());
    } else {
        lines.push(format!(
            "        samples go to {}",
            paths::xdg_config_dir().join("abmx/samples").display()
        ));
    }

    note("Summary", lines.join("\n"))?;

    if !config.labels.accounts.is_empty() {
        note(
            "Author a parser with AI",
            "1. abmx samples\n\
             2. mkdir -p ~/.agents/skills/abmx && abmx skill > ~/.agents/skills/abmx/SKILL.md\n\
             3. prompt your agent:\n\
                \"help me create parser for all my transaction email, use abmx parser authoring skill\"\n\
             4. abmx sync --dry-run",
        )?;
    }

    Ok(())
}

fn resolve_user_path(value: &str) -> PathBuf {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    let expanded = paths::expand_tilde(Path::new(value));
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    }
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes)
        .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
    write_secret_file(path, text)
}
