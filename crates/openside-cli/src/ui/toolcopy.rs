//! The words for tool calls: the same sentences as the web's `components/toolCopy.ts`
//! ("Opened Acme", "Reading a page"), so a terminal run reads like the app.

use serde_json::Value;

/// What the stream told us about a call beyond its own arguments.
#[derive(Clone, Debug, Default)]
pub(crate) struct Facts {
    /// Site the browser was on.
    pub(crate) host: Option<String>,
    /// Title of the page a navigation opened.
    pub(crate) title: Option<String>,
    /// Name of the element a click or fill acted on.
    pub(crate) label: Option<String>,
}

pub(crate) struct Described {
    pub(crate) title: String,
    /// One quiet extra fact (the site of a page title, the command that ran).
    pub(crate) detail: Option<String>,
}

const TITLE_LIMIT: usize = 60;

fn clip(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > limit {
        let cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    } else {
        flat
    }
}

fn quoted(label: Option<&str>) -> Option<String> {
    label
        .filter(|label| !label.is_empty())
        .map(|label| format!("“{}”", clip(label, 48)))
}

/// "Email address" → "email address"; a name with its own capitals ("Gmail ID") stays.
fn field(label: &str) -> String {
    let text = clip(label, 40);
    let mut words = text.split(' ');
    let simple = words.next().is_some_and(|first| {
        let mut chars = first.chars();
        chars.next().is_some_and(char::is_uppercase) && chars.all(char::is_lowercase)
    }) && words.all(|word| word.chars().all(char::is_lowercase));
    if simple {
        let mut chars = text.chars();
        chars
            .next()
            .map(|first| first.to_lowercase().chain(chars).collect())
            .unwrap_or(text)
    } else {
        text
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn first_line(text: Option<&str>) -> Option<String> {
    text.and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
        .map(|line| clip(line, 90))
}

fn base_name(path: Option<&str>) -> Option<String> {
    path.and_then(|path| path.rsplit(['/', '\\']).find(|part| !part.is_empty()))
        .map(str::to_owned)
}

/// `https://www.acme.com/x` → `acme.com`; anything else → `None`.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.trim_start_matches("www.");
    (!host.is_empty()).then(|| host.to_owned())
}

/// `my_server__fetch_page` → "fetch page".
fn humanize(name: &str) -> String {
    let last = name
        .split("__")
        .filter(|part| !part.is_empty())
        .last()
        .unwrap_or(name);
    let words: String = last
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let words = words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if words.is_empty() {
        "a tool".to_owned()
    } else {
        words
    }
}

fn site(args: &Value, facts: &Facts) -> Option<String> {
    facts
        .host
        .clone()
        .or_else(|| str_arg(args, "url").and_then(host_of))
        .or_else(|| str_arg(args, "site").and_then(|site| host_of(&format!("https://{site}"))))
}

/// A sentence in the agent's voice: past tense once the call finished, present while it runs.
pub(crate) fn describe(name: &str, args: &Value, facts: &Facts, done: bool) -> Described {
    let pick = |past: String, present: String| if done { past } else { present };
    let fixed = |past: &str, present: &str| pick(past.to_owned(), present.to_owned());
    let file = || base_name(str_arg(args, "path")).unwrap_or_else(|| "a file".to_owned());
    let label = quoted(facts.label.as_deref());
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    let lower = name.to_lowercase();
    let mut detail: Option<String> = None;
    let title = match lower.as_str() {
        "browser_open_tab" | "browser_navigate" => {
            let host = facts
                .host
                .clone()
                .or_else(|| str_arg(args, "url").and_then(host_of));
            let page = facts
                .title
                .as_deref()
                .map(|title| clip(title, TITLE_LIMIT))
                .or_else(|| host.clone());
            if facts.title.is_some() {
                detail = host.clone();
            }
            pick(
                page.map_or_else(
                    || "Opened a page".to_owned(),
                    |page| format!("Opened {page}"),
                ),
                host.map_or_else(
                    || "Opening a page".to_owned(),
                    |host| format!("Opening {host}"),
                ),
            )
        }
        "browser_tabs" => fixed("Checked the open tabs", "Checking the open tabs"),
        "browser_snapshot" | "browser_screenshot" => fixed("Read the page", "Reading the page"),
        "browser_close_tab" => fixed("Closed a tab", "Closing a tab"),
        "browser_act" => match action {
            "click" => pick(
                label.map_or_else(
                    || "Clicked on the page".to_owned(),
                    |label| format!("Clicked {label}"),
                ),
                quoted(facts.label.as_deref()).map_or_else(
                    || "Clicking on the page".to_owned(),
                    |label| format!("Clicking {label}"),
                ),
            ),
            "type" => {
                let target = facts.label.as_deref().map(field);
                let submit = args.get("submit") == Some(&Value::Bool(true));
                pick(
                    format!(
                        "{}{}",
                        target.as_ref().map_or_else(
                            || "Filled in a field".to_owned(),
                            |t| format!("Filled the {t} field")
                        ),
                        if submit { " and submitted" } else { "" }
                    ),
                    target.map_or_else(
                        || "Filling in a field".to_owned(),
                        |t| format!("Filling the {t} field"),
                    ),
                )
            }
            "press" => {
                let key = str_arg(args, "key").unwrap_or("a key");
                pick(format!("Pressed {key}"), format!("Pressing {key}"))
            }
            "select" => pick(
                label.clone().map_or_else(
                    || "Chose an option".to_owned(),
                    |l| format!("Chose an option in {l}"),
                ),
                label.map_or_else(
                    || "Choosing an option".to_owned(),
                    |l| format!("Choosing an option in {l}"),
                ),
            ),
            "hover" => pick(
                label.clone().map_or_else(
                    || "Hovered over the page".to_owned(),
                    |l| format!("Hovered over {l}"),
                ),
                label.map_or_else(
                    || "Hovering over the page".to_owned(),
                    |l| format!("Hovering over {l}"),
                ),
            ),
            "focus" => pick(
                label.clone().map_or_else(
                    || "Selected a field".to_owned(),
                    |l| format!("Selected {l}"),
                ),
                label.map_or_else(
                    || "Selecting a field".to_owned(),
                    |l| format!("Selecting {l}"),
                ),
            ),
            _ => fixed("Interacted with the page", "Interacting with the page"),
        },
        "browser_pointer" => match action {
            "double_click" => fixed("Double-clicked on the page", "Double-clicking on the page"),
            "type" => fixed("Typed on the page", "Typing on the page"),
            "scroll" => fixed("Scrolled the page", "Scrolling the page"),
            _ => fixed("Clicked on the page", "Clicking on the page"),
        },
        "browser_repl" => fixed("Worked on the page", "Working on the page"),
        "browser_space_reset" => fixed("Reset the browser", "Resetting the browser"),
        "read" => pick(format!("Read {}", file()), format!("Reading {}", file())),
        "write" => pick(format!("Saved {}", file()), format!("Saving {}", file())),
        "edit" => pick(format!("Edited {}", file()), format!("Editing {}", file())),
        "bash" => {
            detail = first_line(str_arg(args, "command"));
            fixed("Ran a command", "Running a command")
        }
        "request_human_step" => fixed("Asked you to take a step", "Waiting for you"),
        "ask_user" => {
            detail = first_line(str_arg(args, "question"));
            fixed("Asked you a question", "Asking you a question")
        }
        "request_login" => fixed("Asked you to sign in", "Waiting for you to sign in"),
        "request_choice" => fixed("Asked you to choose", "Waiting for your choice"),
        "wait_for_approval" => fixed("Waited for your approval", "Waiting for your approval"),
        "request_otp" => fixed("Asked you for a code", "Waiting for your code"),
        "register_authenticator" => fixed("Set up an authenticator", "Setting up an authenticator"),
        "memory_search" => {
            detail = first_line(str_arg(args, "query"));
            fixed("Checked memory", "Checking memory")
        }
        "memory_write" => fixed("Remembered something", "Remembering something"),
        "skill" => {
            let name = str_arg(args, "name");
            pick(
                name.map_or_else(
                    || "Loaded a skill".to_owned(),
                    |n| format!("Loaded the {n} skill"),
                ),
                name.map_or_else(
                    || "Loading a skill".to_owned(),
                    |n| format!("Loading the {n} skill"),
                ),
            )
        }
        "vault_login" => {
            let on = site(args, facts).unwrap_or_else(|| "the page".to_owned());
            pick(
                format!("Filled your saved login on {on}"),
                format!("Filling your saved login on {on}"),
            )
        }
        "vault_otp" => {
            let on = site(args, facts).unwrap_or_else(|| "the page".to_owned());
            pick(
                format!("Entered a saved code on {on}"),
                format!("Entering a saved code on {on}"),
            )
        }
        "vault_passkey" => {
            if action == "register" {
                let on = site(args, facts).unwrap_or_else(|| "this site".to_owned());
                pick(
                    format!("Created a passkey for {on}"),
                    format!("Creating a passkey for {on}"),
                )
            } else {
                let on = site(args, facts).unwrap_or_else(|| "the site".to_owned());
                pick(
                    format!("Signed in to {on} with a passkey"),
                    format!("Signing in to {on} with a passkey"),
                )
            }
        }
        "delegate" => {
            detail = first_line(str_arg(args, "task"));
            fixed("Asked a helper", "Asking a helper")
        }
        "create_task" => {
            detail = first_line(str_arg(args, "goal"));
            fixed("Started a background task", "Starting a background task")
        }
        "steer_task" => fixed("Updated a background task", "Updating a background task"),
        "cancel_task" => fixed(
            "Cancelled a background task",
            "Cancelling a background task",
        ),
        "get_tasks" => fixed(
            "Checked on background tasks",
            "Checking on background tasks",
        ),
        _ => {
            let words = humanize(name);
            if ["search", "grep", "glob", "find"]
                .iter()
                .any(|word| lower.contains(word))
            {
                fixed("Searched", "Searching")
            } else {
                pick(format!("Used {words}"), format!("Using {words}"))
            }
        }
    };
    Described { title, detail }
}
