use std::future::Future;
use std::io::{IsTerminal, Write};
use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use openside_proto::{
    AccountsResponse, CreateOAuthAccountRequest, OAuthAccount, OAuthInputRequest, OAuthMethod,
    OAuthProviderList, OAuthTransaction, OAuthTransactionStatus, ReloginOAuthAccountRequest,
    SubscriptionAccount, UpdateOAuthAccountRequest,
};
use serde_json::json;
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    client::{encode_segment, Client},
    emit, output_error,
};

const OAUTH: &str = "oauth";
const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Subcommand)]
pub(crate) enum SubscriptionCommand {
    /// List the providers this daemon can sign in to with OAuth and their sign-in methods.
    Providers,
    /// Create an isolated CLI home and print the external sign-in command (claude-code,
    /// codex-cli), or sign in to a new OAuth account (`oauth --provider <provider>`).
    Add {
        source: String,
        #[arg(long)]
        label: String,
        /// OAuth provider from `providers`; required for `oauth`.
        #[arg(long, required_if_eq("source", "oauth"))]
        provider: Option<String>,
        /// OAuth sign-in method; default: the provider's first method.
        #[arg(long, value_enum)]
        method: Option<Method>,
        /// Append the new OAuth account to the end of its provider's pool once signed in.
        #[arg(long)]
        pool: bool,
    },
    /// Sign an existing OAuth account in again: `login oauth <provider>/<account_id>`.
    Login {
        source: String,
        account_id: String,
        #[arg(long, value_enum)]
        method: Option<Method>,
    },
    /// Show login readiness and the exact external sign-in command for this account
    /// (`oauth <provider>/<account_id>` for OAuth accounts).
    Show { source: String, account_id: String },
    /// Enable one existing subscription account.
    Enable { source: String, account_id: String },
    /// Disable one existing subscription account.
    Disable { source: String, account_id: String },
    /// Remove a subscription account; never delete the default user CLI home.
    Rm { source: String, account_id: String },
    /// Change OAuth account metadata: `update oauth <provider>/<account_id>`.
    Update {
        source: String,
        account_id: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        reserve_percent: Option<u8>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Method {
    Browser,
    Device,
}

impl From<Method> for OAuthMethod {
    fn from(method: Method) -> Self {
        match method {
            Method::Browser => OAuthMethod::Browser,
            Method::Device => OAuthMethod::Device,
        }
    }
}

fn method_name(method: &OAuthMethod) -> &'static str {
    match method {
        OAuthMethod::Browser => "browser",
        OAuthMethod::Device => "device",
    }
}

pub(crate) async fn run<W: Write, E: Write>(
    command: Option<&SubscriptionCommand>,
    client: &Client,
    json_output: bool,
    stdout: &mut W,
    stderr: &mut E,
    interrupted: &mut bool,
) -> Result<(), String> {
    let Some(command) = command else {
        return list(client, json_output, stdout).await;
    };
    let (source, id) = match command {
        SubscriptionCommand::Providers => (OAUTH, None),
        SubscriptionCommand::Add { source, .. } => (source.as_str(), None),
        SubscriptionCommand::Login {
            source, account_id, ..
        }
        | SubscriptionCommand::Update {
            source, account_id, ..
        }
        | SubscriptionCommand::Show { source, account_id }
        | SubscriptionCommand::Enable { source, account_id }
        | SubscriptionCommand::Disable { source, account_id }
        | SubscriptionCommand::Rm { source, account_id } => {
            (source.as_str(), Some(account_id.as_str()))
        }
    };
    if source == OAUTH {
        return oauth(
            command,
            id,
            client,
            json_output,
            stdout,
            stderr,
            interrupted,
        )
        .await;
    }
    if !matches!(source, "claude-code" | "codex-cli") {
        return Err("source must be claude-code, codex-cli or oauth".into());
    }
    let mut path = format!("/v1/settings/subscriptions/{source}");
    if let Some(id) = id {
        path.push('/');
        path.push_str(crate::models::component(id)?);
    }
    let account: SubscriptionAccount = match command {
        SubscriptionCommand::Add {
            provider: None,
            method: None,
            pool: false,
            label,
            ..
        } => {
            client
                .json("POST", &path, Some(&json!({"label":label})))
                .await?
        }
        SubscriptionCommand::Add { .. } => {
            return Err("--provider, --method and --pool apply only to `add oauth`".into());
        }
        SubscriptionCommand::Show { .. } => client.get(&path).await?,
        SubscriptionCommand::Enable { .. } => {
            client
                .json("PATCH", &path, Some(&json!({"enabled":true})))
                .await?
        }
        SubscriptionCommand::Disable { .. } => {
            client
                .json("PATCH", &path, Some(&json!({"enabled":false})))
                .await?
        }
        SubscriptionCommand::Rm { .. } => {
            client.send("DELETE", &path).await?;
            if !json_output {
                writeln!(stdout, "Subscription account removed").map_err(output_error)?;
            }
            return Ok(());
        }
        SubscriptionCommand::Providers
        | SubscriptionCommand::Login { .. }
        | SubscriptionCommand::Update { .. } => {
            return Err(
                "`login` and `update` apply only to OAuth accounts: oauth <provider>/<account_id>"
                    .into(),
            );
        }
    };
    if json_output {
        emit(stdout, &account)
    } else {
        writeln!(
            stdout,
            "{}\t{}\t{}\tinstalled={}\tsigned_in={}\tenabled={}\n{}\nLog in: {}",
            account.source,
            account.account_id,
            account.label,
            account.installed,
            account.logged_in,
            account.enabled,
            account.reason,
            account.login_command
        )
        .map_err(output_error)
    }
}

/// `<provider>/<account_id>` as the account route path.
fn oauth_account_path(target: &str) -> Result<String, String> {
    let (provider, id) = target
        .split_once('/')
        .ok_or("OAuth accounts are named <provider>/<account_id>")?;
    Ok(format!(
        "/v1/settings/oauth/accounts/{}/{}",
        crate::models::component(provider)?,
        crate::models::component(id)?
    ))
}

async fn oauth<W: Write, E: Write>(
    command: &SubscriptionCommand,
    target: Option<&str>,
    client: &Client,
    json_output: bool,
    stdout: &mut W,
    stderr: &mut E,
    interrupted: &mut bool,
) -> Result<(), String> {
    let path = target.map(oauth_account_path).transpose()?;
    let path = path.as_deref().unwrap_or_default();
    let update = |label: Option<&String>, enabled, reserve_percent| UpdateOAuthAccountRequest {
        label: label.cloned(),
        enabled,
        reserve_percent,
    };
    let account: OAuthAccount = match command {
        SubscriptionCommand::Providers => return providers(client, json_output, stdout).await,
        SubscriptionCommand::Add {
            label,
            provider,
            method,
            pool,
            ..
        } => {
            let provider = provider
                .as_deref()
                .ok_or("`add oauth` needs --provider <provider>")?;
            let request = CreateOAuthAccountRequest {
                label: label.clone(),
                join_pool: *pool,
                method: method.map(Into::into),
            };
            let path = format!(
                "/v1/settings/oauth/accounts/{}",
                crate::models::component(provider)?
            );
            let transaction = client.json("POST", &path, Some(&request)).await?;
            return sign_in(
                client,
                transaction,
                !*pool,
                json_output,
                stdout,
                stderr,
                interrupted,
            )
            .await;
        }
        SubscriptionCommand::Login { method, .. } => {
            let request = ReloginOAuthAccountRequest {
                method: method.map(Into::into),
            };
            let transaction = client
                .json("POST", &format!("{path}/relogin"), Some(&request))
                .await?;
            return sign_in(
                client,
                transaction,
                false,
                json_output,
                stdout,
                stderr,
                interrupted,
            )
            .await;
        }
        SubscriptionCommand::Show { .. } => client.get(path).await?,
        SubscriptionCommand::Enable { .. } => {
            client
                .json("PATCH", path, Some(&update(None, Some(true), None)))
                .await?
        }
        SubscriptionCommand::Disable { .. } => {
            client
                .json("PATCH", path, Some(&update(None, Some(false), None)))
                .await?
        }
        SubscriptionCommand::Update {
            label,
            reserve_percent,
            ..
        } => {
            if label.is_none() && reserve_percent.is_none() {
                return Err("`update oauth` needs --label or --reserve-percent".into());
            }
            client
                .json(
                    "PATCH",
                    path,
                    Some(&update(label.as_ref(), None, *reserve_percent)),
                )
                .await?
        }
        SubscriptionCommand::Rm { .. } => {
            client.send("DELETE", path).await?;
            if !json_output {
                writeln!(stdout, "OAuth account removed").map_err(output_error)?;
            }
            return Ok(());
        }
    };
    if json_output {
        emit(stdout, &account)
    } else {
        print_account(stdout, &account)
    }
}

fn print_account<W: Write>(stdout: &mut W, account: &OAuthAccount) -> Result<(), String> {
    writeln!(
        stdout,
        "{}\t{}/{}\t{}\tenabled={}\tavailable={}\treserve={}%\n{}",
        account.source,
        account.provider,
        account.account_id,
        account.label,
        account.enabled,
        account.available,
        account.reserve_percent,
        account.reason
    )
    .map_err(output_error)
}

async fn providers<W: Write>(client: &Client, json: bool, stdout: &mut W) -> Result<(), String> {
    let list: OAuthProviderList = client.get("/v1/settings/oauth/providers").await?;
    if json {
        return emit(stdout, &list);
    }
    let names = |methods: &[OAuthMethod]| {
        if methods.is_empty() {
            "-".to_owned()
        } else {
            methods
                .iter()
                .map(method_name)
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    writeln!(stdout, "PROVIDER\tNAME\tMETHODS\tPASTE CODE").map_err(output_error)?;
    for provider in &list.providers {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}",
            provider.provider,
            provider.name,
            names(&provider.methods),
            names(&provider.manual_input_methods)
        )
        .map_err(output_error)?;
    }
    Ok(())
}

/// Follows a create or re-login transaction to its end. `pool_hint` names how to pool an
/// account that was created without `--pool`, since OAuth accounts never join one on their own.
async fn sign_in<W: Write, E: Write>(
    client: &Client,
    transaction: OAuthTransaction,
    pool_hint: bool,
    json_output: bool,
    stdout: &mut W,
    stderr: &mut E,
    interrupted: &mut bool,
) -> Result<(), String> {
    let mut terminal = terminal_prompt;
    let ask: Option<&mut dyn FnMut() -> Prompt> = if std::io::stdin().is_terminal() {
        Some(&mut terminal)
    } else {
        None
    };
    let interrupt = async {
        // Without a SIGINT handler the default action (exit) stays in force.
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let transaction =
        match follow(client, transaction, stderr, ask, interrupt, POLL_INTERVAL).await? {
            Followed::Finished(transaction) => transaction,
            Followed::Interrupted => {
                *interrupted = true;
                return Ok(());
            }
        };
    let account = match (&transaction.status, &transaction.account) {
        (OAuthTransactionStatus::Ready, Some(account)) => account,
        _ => {
            let reason = transaction.reason.as_deref().unwrap_or("oauth_failed");
            return Err(match oauth_message(reason) {
                Some(message) => format!("{message} ({reason})"),
                None => format!("sign-in did not finish ({reason})"),
            });
        }
    };
    if json_output {
        return emit(stdout, &transaction);
    }
    print_account(stdout, account)?;
    if pool_hint {
        writeln!(
            stdout,
            "Not in a pool yet: add pi-oauth:{} with `openside models pool set {} ...`",
            account.account_id, account.provider
        )
        .map_err(output_error)?;
    }
    Ok(())
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum Followed {
    Finished(OAuthTransaction),
    /// Ctrl-C: the transaction was cancelled on the daemon.
    Interrupted,
}

/// A pending answer from the code prompt.
pub(crate) struct Prompt {
    answer: oneshot::Receiver<Result<Zeroizing<String>, String>>,
    /// Terminal settings from before the prompt: the reading thread cannot be stopped, so a
    /// prompt abandoned for Ctrl-C or a finished browser sign-in restores echo itself.
    #[cfg(unix)]
    saved: Option<nix::sys::termios::Termios>,
}

impl Drop for Prompt {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = &self.saved {
            let _ = nix::sys::termios::tcsetattr(
                std::io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                saved,
            );
        }
    }
}

#[cfg(test)]
impl Prompt {
    pub(crate) fn answered(code: &str) -> Self {
        let (sender, answer) = oneshot::channel();
        let _ = sender.send(Ok(Zeroizing::new(code.to_owned())));
        Self {
            answer,
            #[cfg(unix)]
            saved: None,
        }
    }
}

const CODE_PROMPT: &str =
    "Paste the code or redirect URL (never a token), or press Enter to finish in the browser";

/// Reads the code on its own thread so polling, Ctrl-C and a browser callback that finishes
/// first are still noticed while the person types.
fn terminal_prompt() -> Prompt {
    #[cfg(unix)]
    let saved = nix::sys::termios::tcgetattr(std::io::stdin()).ok();
    let (sender, answer) = oneshot::channel();
    std::thread::spawn(move || {
        let code =
            crate::prompt::read_hidden(CODE_PROMPT, &mut std::io::stdin(), &mut std::io::stderr())
                .map(Zeroizing::new);
        let _ = sender.send(code);
    });
    Prompt {
        answer,
        #[cfg(unix)]
        saved,
    }
}

fn is_terminal_status(status: &OAuthTransactionStatus) -> bool {
    matches!(
        status,
        OAuthTransactionStatus::Ready
            | OAuthTransactionStatus::Failed
            | OAuthTransactionStatus::Cancelled
            | OAuthTransactionStatus::Expired
    )
}

fn render_authorization(client: &Client, transaction: &OAuthTransaction) -> Option<String> {
    let authorization = transaction.authorization.as_ref()?;
    let mut text = format!(
        "Sign in to {}: {}",
        crate::ui::clean(&transaction.provider),
        crate::ui::clean(&authorization.url)
    );
    if let Some(launch) = authorization
        .launch_url
        .as_deref()
        .filter(|_| !client.is_remote())
    {
        text.push_str(&format!(
            "\nOn this computer you can also open: {}",
            crate::ui::clean(launch)
        ));
    }
    if let Some(instructions) = &authorization.instructions {
        text.push('\n');
        text.push_str(&crate::ui::clean(instructions));
    }
    if let Some(code) = &authorization.user_code {
        text.push_str(&format!("\nCode: {}", crate::ui::clean(code)));
    }
    Some(text)
}

/// Polls the transaction every `poll` until it ends. Sign-in guidance goes to `stderr` once per
/// change; `ask` (a terminal) is offered the code prompt each time the daemon starts accepting a
/// pasted code. `interrupt` cancels the transaction.
pub(crate) async fn follow<E: Write>(
    client: &Client,
    mut transaction: OAuthTransaction,
    stderr: &mut E,
    mut ask: Option<&mut dyn FnMut() -> Prompt>,
    interrupt: impl Future<Output = ()>,
    poll: Duration,
) -> Result<Followed, String> {
    enum Next {
        Interrupt,
        Answer(Result<Zeroizing<String>, String>),
        Poll,
    }
    tokio::pin!(interrupt);
    let base = format!(
        "/v1/settings/oauth/transactions/{}",
        encode_segment(&transaction.transaction_id)
    );
    let mut shown = None::<String>;
    let mut asked = false;
    let mut exchanging = false;
    let mut prompt = None::<Prompt>;
    loop {
        if is_terminal_status(&transaction.status) {
            return Ok(Followed::Finished(transaction));
        }
        let guidance = render_authorization(client, &transaction);
        if guidance.is_some() && guidance != shown {
            if prompt.is_some() {
                writeln!(stderr).map_err(output_error)?;
            }
            writeln!(stderr, "{}", guidance.as_deref().unwrap_or_default())
                .map_err(output_error)?;
            shown = guidance;
        }
        if matches!(transaction.status, OAuthTransactionStatus::Exchanging) && !exchanging {
            exchanging = true;
            writeln!(stderr, "Finishing sign-in…").map_err(output_error)?;
        }
        if transaction.manual_input {
            if !asked {
                asked = true;
                match ask.as_mut() {
                    Some(ask) => prompt = Some(ask()),
                    None => writeln!(
                        stderr,
                        "Finish signing in in the browser; this terminal cannot take a pasted code."
                    )
                    .map_err(output_error)?,
                }
            }
        } else {
            asked = false;
            prompt = None;
        }
        let next = {
            let answer = async {
                match prompt.as_mut() {
                    Some(prompt) => (&mut prompt.answer)
                        .await
                        .unwrap_or_else(|_| Err("could not read the answer".into())),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                () = &mut interrupt => Next::Interrupt,
                answer = answer => Next::Answer(answer),
                () = tokio::time::sleep(poll) => Next::Poll,
            }
        };
        match next {
            Next::Interrupt => {
                // Restores terminal echo before anything else is written.
                drop(prompt.take());
                // The SIGINT handler stays installed, so without this a second Ctrl-C while
                // the cancel request is in flight would do nothing.
                tokio::spawn(async {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        std::process::exit(130);
                    }
                });
                writeln!(stderr).map_err(output_error)?;
                client.send("POST", &format!("{base}/cancel")).await?;
                writeln!(stderr, "Sign-in cancelled").map_err(output_error)?;
                return Ok(Followed::Interrupted);
            }
            Next::Answer(answer) => {
                prompt = None;
                match answer {
                    Ok(code) if !code.is_empty() => {
                        let mut request = OAuthInputRequest {
                            code: code.as_str().to_owned(),
                        };
                        let sent = client
                            .json("POST", &format!("{base}/input"), Some(&request))
                            .await;
                        request.code.zeroize();
                        match sent {
                            Ok(next) => transaction = next,
                            // The browser may have finished first; the next poll says how it ended.
                            Err(error) => {
                                writeln!(stderr, "openside: {error}").map_err(output_error)?
                            }
                        }
                    }
                    Ok(_) => writeln!(stderr, "Waiting for the browser sign-in to finish.")
                        .map_err(output_error)?,
                    Err(error) => writeln!(
                        stderr,
                        "openside: {error}; waiting for the browser sign-in to finish."
                    )
                    .map_err(output_error)?,
                }
            }
            Next::Poll => transaction = client.get(&base).await?,
        }
    }
}

/// Concise English for the OAuth error codes and failed-transaction reasons.
pub(crate) fn oauth_message(code: &str) -> Option<&'static str> {
    Some(match code {
        "oauth_provider_unknown" => {
            "this daemon cannot sign in to that provider; see `openside models accounts providers`"
        }
        "oauth_flow_unsupported" => {
            "that sign-in method is not available for this provider; see `openside models accounts providers`"
        }
        "oauth_manual_input_not_allowed" => "this sign-in is not waiting for a pasted code",
        "oauth_transaction_finished" => "this sign-in has already finished",
        "oauth_transaction_unknown" => {
            "sign-in not found; it may have expired or belong to another device"
        }
        "oauth_busy" => "another sign-in is already in progress; finish or cancel it first",
        "unknown_account" => "no such OAuth account",
        "vault_unavailable" => "the vault is locked; run `openside vault unlock` and try again",
        "person_device_required" => {
            "OAuth sign-in needs a person's device or the daemon's local socket, not an agent client or channel"
        }
        "oauth_callback_busy" => {
            "the sign-in callback port on the daemon's computer is busy; close the other sign-in or use --method device where available"
        }
        "oauth_invalid_grant" => "the provider rejected the sign-in; start it again",
        "oauth_failed" => "sign-in failed",
        "oauth_helper_unavailable" => {
            "the daemon could not start its sign-in helper; run `openside doctor` on the daemon host"
        }
        "oauth_cancelled" => "sign-in cancelled",
        "oauth_expired" => "sign-in expired; start it again",
        "oauth_account_removed" => "the account was removed before sign-in finished",
        _ => return None,
    })
}

async fn list<W: Write>(client: &Client, json: bool, stdout: &mut W) -> Result<(), String> {
    let response: AccountsResponse = client.get("/v1/accounts").await?;
    if json {
        return emit(stdout, &response);
    }
    if let Some(ui) = crate::ui::pretty() {
        return write!(
            stdout,
            "{}",
            crate::ui::accounts(ui, &response, crate::ui::now_secs())
        )
        .map_err(output_error);
    }
    writeln!(
        stdout,
        "SOURCE\tACCOUNT\tPROVIDER\tMODELS\tUSAGE\tBLOCKED UNTIL (MS)"
    )
    .map_err(output_error)?;
    for source in &response.sources {
        if source.accounts.is_empty() {
            writeln!(stdout, "{}\t{}\t-\t-\t-\t-", source.name, source.reason)
                .map_err(output_error)?;
        }
        for account in &source.accounts {
            let usage = account
                .usage_windows
                .iter()
                .map(|window| match window.used_percent {
                    Some(used) => format!("{}: {used:.1}%", window.name),
                    None => format!("{}: unknown", window.name),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let blocked = account
                .blocked_until_ms
                .map(|value| format!("{value:.0}"))
                .unwrap_or_else(|| "-".into());
            writeln!(
                stdout,
                "{}\t{}\t{}\t{}\t{}\t{}",
                source.name,
                account.label,
                account.provider,
                account.models.join(", "),
                if usage.is_empty() { "-" } else { &usage },
                blocked
            )
            .map_err(output_error)?;
        }
    }
    if response.truncated {
        writeln!(
            stdout,
            "Account metadata truncated at the documented limits"
        )
        .map_err(output_error)?;
    }
    Ok(())
}
