use openside_proto::*;
use openside_sessions::SessionState;
use serde_json::Value;

fn notice(id: &str, text: impl Into<String>, tone: NoticeTone) -> Block {
    Block::Notice {
        id: id.into(),
        text: text.into(),
        tone,
        placement: NoticePlacement::Inline,
    }
}
fn action(id: &str, label: &str, target: ActionTarget, primary: bool) -> Action {
    Action {
        id: id.into(),
        label: label.into(),
        style: if primary {
            ActionStyle::Primary
        } else {
            ActionStyle::Secondary
        },
        target,
    }
}
fn answer(id: &str, label: &str, value: TicketAnswer, primary: bool) -> Action {
    action(
        id,
        label,
        ActionTarget::TicketResolve { answer: value },
        primary,
    )
}
fn field(id: &str, label: &str, secret: bool, origin: Option<String>) -> InputField {
    InputField {
        id: id.into(),
        label: label.into(),
        kind: if secret {
            InputKind::Password
        } else {
            InputKind::Text
        },
        secret,
        otp: false,
        card: None,
        required: true,
        autocomplete: None,
        placeholder: None,
        validation: InputValidation::Text,
        max_length: 4096,
        initial: None,
        suggestions: vec![],
        target_origin: origin,
    }
}
fn fields_answer(encoding: FieldAnswerEncoding, save: Option<String>) -> TicketAnswer {
    TicketAnswer::Fields {
        block_ids: vec!["fields".into()],
        encoding,
        save_field_id: save,
    }
}
fn approved(ticket: &Ticket) -> bool {
    ticket
        .resolution
        .as_ref()
        .is_some_and(|v| v == &Value::Bool(true) || v.get("approved") == Some(&Value::Bool(true)))
}
fn failed(value: Option<&Value>) -> bool {
    value.is_some_and(|v| {
        ["next", "status", "error"].iter().any(|key| {
            v.get(*key).and_then(Value::as_str).is_some_and(|s| {
                matches!(
                    s,
                    "error"
                        | "failed"
                        | "fill_failed"
                        | "setup_key_lost"
                        | "indeterminate"
                        | "registration_indeterminate"
                        | "interrupted"
                        | "partial_fill"
                )
            })
        })
    })
}

fn receipt(ticket: &Ticket) -> (CardStatus, &'static str) {
    let result = ticket.resolution.as_ref();
    let status = result.and_then(|v| v.get("status")).and_then(Value::as_str);
    let declined = result.is_some_and(|v| {
        v == &Value::Bool(false) || v.get("approved") == Some(&Value::Bool(false))
    });
    if ticket.kind == TicketKind::Confirm && declined {
        return (CardStatus::Done, "Declined");
    }
    if status == Some("registered") && (ticket.otp.is_some() || ticket.otp_register.is_some()) {
        return (CardStatus::Done, "Authenticator saved");
    }
    if ticket.form.is_some() {
        return if status == Some("filled") {
            (CardStatus::Done, "Details filled — page not submitted")
        } else {
            (CardStatus::Failed, "Fill outcome unavailable")
        };
    }
    match ticket.kind {
        TicketKind::Hire if status == Some("hired") => (CardStatus::Done, "Teammate hired"),
        TicketKind::Hire if status == Some("later") => (CardStatus::Done, "Saved for later"),
        TicketKind::Hire if status == Some("declined") => (CardStatus::Done, "Proposal declined"),
        TicketKind::Login if status == Some("alternative") => {
            (CardStatus::Done, "Provider selected")
        }
        TicketKind::Login if matches!(status, Some("submitted" | "filled_from_vault")) => {
            (CardStatus::Done, "Details sent")
        }
        TicketKind::Choice if status == Some("chosen") => (CardStatus::Done, "Choice selected"),
        TicketKind::SecretInput if status == Some("filled") => (
            CardStatus::Done,
            if ticket.otp.is_some() {
                "Code sent"
            } else {
                "Details filled"
            },
        ),
        TicketKind::Info => (CardStatus::Done, "Answer received"),
        TicketKind::DeviceApproval | TicketKind::Takeover if status == Some("done") => {
            (CardStatus::Done, "Step completed")
        }
        TicketKind::Confirm if approved(ticket) => (CardStatus::Done, "Approved"),
        _ => (CardStatus::Failed, "Outcome unavailable"),
    }
}

fn row(label: &str, text: impl Into<String>) -> KvRow {
    KvRow {
        label: label.into(),
        value: CardValue::Text { text: text.into() },
    }
}

fn rows(id: &str, rows: Vec<KvRow>) -> Block {
    Block::KvRows {
        id: id.into(),
        rows,
        total: None,
    }
}

/// Explanation rows that only make sense while the person can still answer; receipts drop them.
const ABOUT: &str = "about";

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Plain words for the common cron shapes; anything else shows the expression the agent proposed.
fn cron_words(expr: &str) -> String {
    let expr = expr.trim();
    let alias = match expr {
        "@hourly" => Some("Every hour"),
        "@daily" | "@midnight" => Some("Every day at 00:00"),
        "@weekly" => Some("Every Sunday at 00:00"),
        "@monthly" => Some("On the 1st of every month at 00:00"),
        "@yearly" | "@annually" => Some("Every January 1 at 00:00"),
        _ => None,
    };
    if let Some(alias) = alias {
        return alias.into();
    }
    let raw = || format!("On the cron schedule {expr}");
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = parts[..] else {
        return raw();
    };
    if !numeric(minute) || month != "*" {
        return raw();
    }
    if hour == "*" && day == "*" && weekday == "*" {
        return format!("Every hour at :{minute:0>2}");
    }
    if !numeric(hour) {
        return raw();
    }
    let time = format!("{hour:0>2}:{minute:0>2}");
    match (day, weekday) {
        ("*", "*") => format!("Every day at {time}"),
        ("*", "1-5") => format!("Weekdays at {time}"),
        ("*", "0,6" | "6,0") => format!("Weekends at {time}"),
        ("*", w) if numeric(w) => match w.parse::<usize>().ok().and_then(|n| WEEKDAYS.get(n % 7)) {
            Some(name) => format!("Every {name} at {time}"),
            None => raw(),
        },
        (d, "*") if numeric(d) => format!("On day {d} of every month at {time}"),
        _ => raw(),
    }
}

fn interval_words(secs: u64) -> String {
    for (unit, size) in [("day", 86_400), ("hour", 3_600), ("minute", 60)] {
        if secs % size == 0 {
            let count = secs / size;
            return if count == 1 {
                format!("Every {unit}")
            } else {
                format!("Every {count} {unit}s")
            };
        }
    }
    if secs == 1 {
        "Every second".into()
    } else {
        format!("Every {secs} seconds")
    }
}

/// Longest routine message shown in a card row; a longer one keeps the full instructions too.
const ROUTINE_PROMPT_SHOWN: usize = 1_000;
/// Longest agent-written reason shown on a sign-in card.
const AGENT_REASON_SHOWN: usize = 200;

/// Clips to `max` characters with an ellipsis; `None` when nothing was cut.
fn clipped(text: &str, max: usize) -> Option<String> {
    text.char_indices()
        .nth(max)
        .map(|(end, _)| format!("{}…", text[..end].trim_end()))
}

/// What a routine proposal would create, read from the validated, secret-screened tool
/// arguments; the flag is false when a row had to be shortened.
fn routine_rows(ticket: &Ticket) -> Option<(Vec<KvRow>, bool)> {
    if !crate::routines::proposals::is_proposal(ticket) {
        return None;
    }
    let request: CreateRoutineRequest =
        serde_json::from_value(ticket.tool.as_ref()?.args.clone()).ok()?;
    let mut out = Vec::new();
    match &request.schedule {
        RoutineSchedule::Cron { expr, timezone } => {
            out.push(row("Schedule", cron_words(expr)));
            out.push(row("Time zone", timezone.as_deref().unwrap_or("UTC")));
        }
        RoutineSchedule::Interval { secs } => out.push(row(
            "Schedule",
            format!("{}, counted from when you approve", interval_words(*secs)),
        )),
    }
    let (target, prompt) = match &request.action {
        RoutineAction::NewSession { prompt, .. } => ("A new conversation each time".into(), prompt),
        RoutineAction::Agent { agent_id, prompt } => {
            (format!("Teammate {agent_id}'s conversation"), prompt)
        }
        RoutineAction::Heartbeat { session_id, prompt } => (
            if *session_id == ticket.session_id {
                "This conversation".into()
            } else {
                "Another existing conversation".into()
            },
            prompt,
        ),
    };
    out.push(row("Runs in", target));
    let shortened = clipped(prompt, ROUTINE_PROMPT_SHOWN);
    let complete = shortened.is_none();
    out.push(row(
        "Message sent",
        shortened.unwrap_or_else(|| prompt.clone()),
    ));
    if request.catch_up == Some(true) {
        out.push(row(
            "Missed runs",
            "Runs once to catch up if the computer was off",
        ));
    }
    Some((out, complete))
}

/// The same projection is used before durable append and when enriching legacy replay.
pub(crate) fn ticket(ticket: &Ticket) -> Card {
    let origin = ticket
        .form
        .as_ref()
        .map(|v| &v.origin)
        .or_else(|| ticket.login.as_ref().map(|v| &v.origin))
        .or_else(|| ticket.choice.as_ref().map(|v| &v.origin))
        .or_else(|| ticket.approval.as_ref().map(|v| &v.origin))
        .or_else(|| ticket.otp_register.as_ref().map(|v| &v.origin))
        .or_else(|| ticket.target.as_ref().and_then(|v| v.origin.as_ref()))
        .and_then(|s| url::Url::parse(s).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.origin().ascii_serialization());
    let name = ticket
        .form
        .as_ref()
        .map(|v| v.site_name.clone())
        .or_else(|| ticket.login.as_ref().map(|v| v.site_name.clone()))
        .or_else(|| ticket.choice.as_ref().map(|v| v.site_name.clone()))
        .or_else(|| ticket.approval.as_ref().map(|v| v.site_name.clone()))
        .unwrap_or_else(|| origin.clone().unwrap_or_else(|| "Local".into()));
    let mut card = Card {
        id: format!("ticket:{}", ticket.id),
        kind: CardKind::Confirmation,
        site: CardSite {
            origin: origin.clone(),
            name,
            favicon_ref: None,
        },
        title: ticket.summary.clone(),
        subtitle: None,
        status: CardStatus::Pending,
        blocks: vec![],
        actions: vec![],
        interaction: CardInteraction::Ask,
        ticket_id: Some(ticket.id.clone()),
        expires_at: Some(ticket.expires_at),
        collapsed_summary: ticket.summary.clone(),
    };
    card.site.favicon_ref = ticket
        .login
        .as_ref()
        .and_then(|v| v.icon.as_deref())
        .or_else(|| ticket.choice.as_ref().and_then(|v| v.icon.as_deref()))
        .or_else(|| ticket.approval.as_ref().and_then(|v| v.icon.as_deref()))
        .and_then(crate::favicons::validated_ref);
    let (routine, routine_complete) =
        routine_rows(ticket).map_or((None, false), |(rows, complete)| (Some(rows), complete));
    // Daemon-authored text for these is replaced by the structured rows below; a routine whose
    // message had to be shortened keeps its full instructions so nothing approved is hidden.
    let structured = routine_complete
        || ticket.otp_register.is_some()
        || ticket.otp.is_some()
        || ticket.login.is_some();
    if let Some(instructions) = ticket.instructions.as_ref().filter(|_| !structured) {
        card.blocks
            .push(notice("instructions", instructions, NoticeTone::Info));
    }
    let explained_at = card.blocks.len();
    let in_tab = ticket.target.as_ref().is_some_and(|t| t.tab_id.is_some());
    let mut about = Vec::new();
    if in_tab {
        about.push(row("Tab", "The agent's browser tab"));
    }
    let login_reason = ticket
        .login
        .as_ref()
        .and(ticket.instructions.as_deref())
        .and_then(|text| text.split_once(crate::tools::login::WHY_REQUESTED))
        .map(|(_, why)| why.trim())
        .filter(|why| !why.is_empty());
    // The model wrote this, so the label says whose words they are.
    if let Some(why) = login_reason {
        let why = clipped(why, AGENT_REASON_SHOWN).unwrap_or_else(|| why.to_owned());
        about.push(row("Agent's reason", why));
    }
    if let Some(form) = &ticket.form {
        card.kind = if form.payment {
            CardKind::PaymentCard
        } else {
            CardKind::Form
        };
        card.subtitle = Some("Fills the fields without submitting the page.".into());
        about.push(row(
            "What happens",
            if form.payment {
                "openside types these details into the page and stops there. Nothing is submitted and no payment is made."
            } else {
                "openside types these details into the page and stops there. Nothing is submitted."
            },
        ));
        card.blocks.push(Block::InputFields {
            id: "fields".into(),
            fields: form.fields.clone(),
        });
        card.actions.push(answer(
            "fill",
            &form.submit_label,
            fields_answer(FieldAnswerEncoding::Form, None),
            true,
        ));
    } else {
        match ticket.kind {
            TicketKind::Login => {
                card.kind = CardKind::SecureLogin;
                if let Some(login) = &ticket.login {
                    let mut next = String::from("Enter your sign-in details below.");
                    if login.accounts.as_ref().is_some_and(|a| !a.is_empty()) {
                        next.push_str(" Or pick one of your saved accounts.");
                    }
                    if !login.alternatives.is_empty() {
                        next.push_str(" Or choose another way to sign in.");
                    }
                    about.push(row("Next step", next));
                    about.push(row(
                        "What happens",
                        "openside types your details into the sign-in form and submits it. The agent is told only whether sign-in worked, not what you typed.",
                    ));
                    let fields = login
                        .fields
                        .iter()
                        .map(|f| {
                            let mut out = field(
                                &f.id,
                                &f.label,
                                f.kind != LoginFieldKind::Checkbox,
                                origin.clone(),
                            );
                            out.kind = if f.kind == LoginFieldKind::Checkbox {
                                InputKind::Checkbox
                            } else if f.masked {
                                InputKind::Password
                            } else {
                                match f.autocomplete.as_deref() {
                                    Some("email") => InputKind::Email,
                                    Some("tel") => InputKind::Tel,
                                    _ => InputKind::Text,
                                }
                            };
                            out.required = f.required;
                            out.autocomplete = f.autocomplete.clone();
                            out.placeholder = f.placeholder.clone();
                            out.suggestions = f.suggestions.clone();
                            out
                        })
                        .collect();
                    card.blocks.push(Block::InputFields {
                        id: "fields".into(),
                        fields,
                    });
                    let save = login.can_save.then(|| "save".to_string());
                    if let Some(id) = &save {
                        let mut f = field(id, "Save to vault", false, None);
                        f.kind = InputKind::Checkbox;
                        f.required = false;
                        card.blocks.push(Block::InputFields {
                            id: "consent".into(),
                            fields: vec![f],
                        });
                    }
                    card.actions.push(answer(
                        "login",
                        &login.submit_label,
                        fields_answer(FieldAnswerEncoding::Login, save),
                        true,
                    ));
                    if let Some(accounts) = &login.accounts {
                        card.blocks.push(Block::ChoiceOptions {
                            id: "accounts".into(),
                            label: Some("Saved accounts".into()),
                            layout: ChoiceLayout::Stack,
                            required: false,
                            options: accounts
                                .iter()
                                .map(|a| ChoiceOption {
                                    id: a.item_id.clone(),
                                    label: a.username_hint.clone(),
                                    description: None,
                                    provider: None,
                                    action: Some(answer(
                                        &format!("account:{}", a.item_id),
                                        &a.username_hint,
                                        TicketAnswer::LoginAccount {
                                            item_id: a.item_id.clone(),
                                        },
                                        false,
                                    )),
                                })
                                .collect(),
                        });
                    }
                    if !login.alternatives.is_empty() {
                        card.blocks.push(Block::ChoiceOptions {
                            id: "alternatives".into(),
                            label: Some("Other ways to sign in".into()),
                            layout: ChoiceLayout::Providers,
                            required: false,
                            options: login
                                .alternatives
                                .iter()
                                .map(|a| ChoiceOption {
                                    id: a.id.clone(),
                                    label: a.label.clone(),
                                    description: None,
                                    provider: a.provider,
                                    action: Some(answer(
                                        &format!("alternative:{}", a.id),
                                        &a.label,
                                        TicketAnswer::LoginAlternative {
                                            alternative_id: a.id.clone(),
                                        },
                                        false,
                                    )),
                                })
                                .collect(),
                        });
                    }
                    if let Some(error) = &login.error_text {
                        card.blocks
                            .push(notice("page-error", error, NoticeTone::Attention));
                    }
                }
            }
            TicketKind::Choice => {
                card.kind = CardKind::LoginChoice;
                if let Some(choice) = &ticket.choice {
                    about.push(row(
                        "What happens",
                        "openside presses the option you pick on the page, then the agent carries on.",
                    ));
                    card.blocks.push(Block::ChoiceOptions {
                        id: "choices".into(),
                        label: Some(choice.question.clone()),
                        layout: ChoiceLayout::Stack,
                        required: true,
                        options: choice
                            .options
                            .iter()
                            .map(|o| ChoiceOption {
                                id: o.id.clone(),
                                label: o.label.clone(),
                                description: o.description.clone(),
                                provider: None,
                                action: None,
                            })
                            .collect(),
                    });
                    card.actions.push(answer(
                        "choose",
                        "Continue",
                        TicketAnswer::SelectedChoice {
                            block_id: "choices".into(),
                            encoding: ChoiceAnswerEncoding::PageOption,
                        },
                        true,
                    ));
                }
            }
            TicketKind::SecretInput => {
                let mut f = field("value", "Secret value", true, origin.clone());
                if let Some(otp) = &ticket.otp {
                    card.kind = CardKind::Otp;
                    f.label = "Code".into();
                    f.kind = InputKind::Text;
                    f.otp = true;
                    f.max_length = 64;
                    f.validation = InputValidation::Otp;
                    f.autocomplete = Some("one-time-code".into());
                    let text = match otp.channel {
                        OtpChannel::Authenticator => "Enter a code from your authenticator app.",
                        OtpChannel::Sms => "Enter the code sent by text message.",
                        OtpChannel::Email => "Enter the code sent by email.",
                        OtpChannel::BackupCode => "Enter a recovery code.",
                        OtpChannel::Unknown => "Enter the code requested by this page.",
                    };
                    card.blocks.push(notice("channel", text, NoticeTone::Info));
                    about.push(row(
                        "What happens",
                        if otp.can_register {
                            "openside types the code into the page; it isn't sent to the agent. You can also save this site's authenticator so later codes fill in by themselves."
                        } else {
                            "openside types the code into the page; it isn't sent to the agent."
                        },
                    ));
                    card.actions.push(answer(
                        "code",
                        "Submit code",
                        TicketAnswer::OtpCode {
                            field_id: "value".into(),
                        },
                        true,
                    ));
                    if otp.can_register {
                        let key = field(
                            "authenticator",
                            "Authenticator setup key",
                            true,
                            origin.clone(),
                        );
                        let mut save = field("save", "Save to vault", false, None);
                        save.kind = InputKind::Checkbox;
                        save.required = false;
                        card.blocks.push(Block::InputFields {
                            id: "authenticator-fields".into(),
                            fields: vec![key, save],
                        });
                        card.actions.push(answer(
                            "authenticator",
                            "Use authenticator",
                            TicketAnswer::OtpAuthenticator {
                                field_id: "authenticator".into(),
                                save_field_id: "save".into(),
                            },
                            false,
                        ));
                    }
                } else {
                    card.kind = CardKind::SecretInput;
                    about.push(row(
                        "What happens",
                        "openside types it straight into the page field. It isn't sent to the agent or kept in the conversation.",
                    ));
                    card.actions.push(answer(
                        "submit",
                        "Submit",
                        fields_answer(FieldAnswerEncoding::Scalar, None),
                        true,
                    ));
                }
                card.blocks.push(Block::InputFields {
                    id: "fields".into(),
                    fields: vec![f],
                });
            }
            TicketKind::Info => {
                card.kind = CardKind::Form;
                if ticket.options.is_empty() {
                    card.blocks.push(Block::InputFields {
                        id: "fields".into(),
                        fields: vec![field("answer", "Answer", false, None)],
                    });
                    card.actions.push(answer(
                        "answer",
                        "Continue",
                        TicketAnswer::Information {
                            field_id: "answer".into(),
                        },
                        true,
                    ));
                } else {
                    card.blocks.push(Block::ChoiceOptions {
                        id: "choices".into(),
                        label: None,
                        layout: ChoiceLayout::Stack,
                        required: true,
                        options: ticket
                            .options
                            .iter()
                            .map(|o| ChoiceOption {
                                id: o.value.clone(),
                                label: o.label.clone(),
                                description: None,
                                provider: None,
                                action: None,
                            })
                            .collect(),
                    });
                    card.actions.push(answer(
                        "answer",
                        "Continue",
                        TicketAnswer::SelectedChoice {
                            block_id: "choices".into(),
                            encoding: ChoiceAnswerEncoding::InfoAnswer,
                        },
                        true,
                    ));
                }
            }
            TicketKind::Hire => {
                card.kind = CardKind::Hire;
                card.site = CardSite {
                    origin: None,
                    name: "Team".into(),
                    favicon_ref: None,
                };
                if let Some(draft) = &ticket.hire {
                    let result = ticket.resolution.as_ref();
                    let name = result
                        .and_then(|v| v["name"].as_str())
                        .unwrap_or(&draft.name);
                    let role = result
                        .and_then(|v| v["role"].as_str())
                        .unwrap_or(&draft.persona.role);
                    card.blocks.push(Block::People {
                        id: "teammate".into(),
                        from: None,
                        to: CardPerson {
                            agent_id: result
                                .and_then(|v| v["agent_id"].as_str())
                                .map(str::to_owned),
                            name: name.into(),
                            initials: draft
                                .initials
                                .clone()
                                .unwrap_or_else(|| name.chars().take(2).collect()),
                            color: draft.color.clone(),
                            role: Some(role.into()),
                        },
                    });
                    let mut name_field = field("name", "Name", false, None);
                    name_field.max_length = 24;
                    name_field.initial = Some(FieldInitial::Text(draft.name.clone()));
                    let mut role_field = field("role", "Role", false, None);
                    role_field.max_length = 60;
                    role_field.initial = Some(FieldInitial::Text(draft.persona.role.clone()));
                    card.blocks.push(Block::InputFields {
                        id: "hire-edit".into(),
                        fields: vec![name_field, role_field],
                    });
                    card.blocks.push(Block::KvRows {
                        id: "responsibilities".into(),
                        rows: vec![
                            KvRow {
                                label: "Responsibilities".into(),
                                value: CardValue::Text {
                                    text: draft.persona.responsibilities.join("; "),
                                },
                            },
                            KvRow {
                                label: "Takes over".into(),
                                value: CardValue::Text {
                                    text: format!(
                                        "{} routines · {} memories",
                                        draft.routine_ids.len(),
                                        draft.memory_ids.len()
                                    ),
                                },
                            },
                            KvRow {
                                label: "Skills".into(),
                                value: CardValue::Text {
                                    text: draft
                                        .skills
                                        .as_ref()
                                        .map(|v| v.join(", "))
                                        .unwrap_or_else(|| "Enabled team skills".into()),
                                },
                            },
                            KvRow {
                                label: "Permissions".into(),
                                value: CardValue::Text {
                                    text: draft
                                        .mode
                                        .map(|v| format!("{v:?}"))
                                        .unwrap_or_else(|| "Team default".into()),
                                },
                            },
                        ],
                        total: None,
                    });
                    for (decision, label, primary) in [
                        ("hire", "Hire", true),
                        ("not_now", "Not now", false),
                        ("never", "Don't suggest this", false),
                    ] {
                        card.actions.push(answer(
                            decision,
                            label,
                            TicketAnswer::Hire {
                                decision: decision.into(),
                                edited_block_id: (decision == "hire").then(|| "hire-edit".into()),
                            },
                            primary,
                        ));
                    }
                } else {
                    card.blocks.push(notice(
                        "missing-proposal",
                        "The original proposal is unavailable. Request a new proposal.",
                        NoticeTone::Attention,
                    ));
                }
            }
            TicketKind::Confirm => {
                let (yes, no) = if ticket.otp_register.is_some() {
                    about.push(row(
                        "What happens",
                        "Save keeps this site's authenticator in your vault so its codes fill in for you. Not now throws the setup key away. The key never appears in the conversation.",
                    ));
                    ("Save", "Not now")
                } else if let Some(routine) = routine {
                    card.blocks.push(rows("routine", routine));
                    about.push(row(
                        "What happens",
                        "Create routine schedules it exactly as shown. Decline creates nothing; a changed routine needs a new proposal.",
                    ));
                    ("Create routine", "Decline")
                } else {
                    about.push(row(
                        "What happens",
                        "Approve lets the agent do this once, exactly as described. Not now stops it and tells the agent you declined.",
                    ));
                    ("Approve", "Not now")
                };
                about.push(row(
                    "If you don't answer",
                    "It's declined automatically when the time runs out.",
                ));
                card.actions.push(answer(
                    "approve",
                    yes,
                    TicketAnswer::Confirm { approved: true },
                    true,
                ));
                card.actions.push(answer(
                    "decline",
                    no,
                    TicketAnswer::Confirm { approved: false },
                    false,
                ));
            }
            TicketKind::DeviceApproval => {
                if let Some(approval) = &ticket.approval {
                    card.blocks
                        .push(notice("approval", &approval.message, NoticeTone::Working));
                    about.push(row(
                        "Next step",
                        "Approve the sign-in on your phone or other trusted device.",
                    ));
                    if let Some(number) = &approval.number {
                        about.push(row("Number to match", number.as_str()));
                    }
                    about.push(row(
                        "What happens",
                        "The agent waits and carries on once the page moves past this step. Cancel stops waiting.",
                    ));
                } else {
                    about.push(row(
                        "Next step",
                        "Finish the step on your device, then press Done.",
                    ));
                    about.push(row(
                        "What happens",
                        "The agent waits until you press Done. Cancel tells it the step didn't happen.",
                    ));
                    card.actions
                        .push(action("done", "Done", ActionTarget::TicketDone, true));
                }
            }
            TicketKind::Takeover => {
                card.kind = CardKind::Takeover;
                if in_tab {
                    about.push(row(
                        "Next step",
                        "Finish this step yourself in the agent's tab, then press Done. Away from the computer, Open remote screen shows the tab here.",
                    ));
                    about.push(row(
                        "What happens",
                        "The agent can't use this tab until you press Done or Cancel. Cancel tells it the step wasn't finished.",
                    ));
                } else {
                    about.push(row(
                        "Next step",
                        "Finish this step on the computer running openside, then press Done.",
                    ));
                    about.push(row(
                        "What happens",
                        "The agent waits until you press Done. Cancel tells it the step wasn't finished.",
                    ));
                }
                card.actions.push(action(
                    "takeover",
                    "Open remote screen",
                    ActionTarget::Takeover,
                    true,
                ));
                card.actions
                    .push(action("done", "Done", ActionTarget::TicketDone, false));
            }
        }
    }
    if !about.is_empty() {
        let at = card
            .blocks
            .iter()
            .position(|b| matches!(b, Block::KvRows { id, .. } if id == "routine"))
            .map_or(explained_at, |i| i + 1);
        card.blocks.insert(at, rows(ABOUT, about));
    }
    if !matches!(ticket.kind, TicketKind::Confirm | TicketKind::Hire) {
        card.actions.push(action(
            "cancel",
            "Cancel",
            ActionTarget::TicketReject,
            false,
        ));
    }
    let bound = origin.is_some() && ticket.target.as_ref().is_some_and(|t| t.tab_id.is_some());
    if matches!(
        ticket.kind,
        TicketKind::Login | TicketKind::Choice | TicketKind::SecretInput
    ) && !bound
    {
        card.blocks.retain(|b| {
            !matches!(b, Block::InputFields { .. } | Block::ChoiceOptions { .. })
                && !matches!(b, Block::KvRows { id, .. } if id == ABOUT)
        });
        card.actions
            .retain(|a| matches!(a.target, ActionTarget::TicketReject));
        card.blocks.push(notice(
            "missing-target",
            "The original field destination is unavailable. Request a new secure step.",
            NoticeTone::Attention,
        ));
    }
    if ticket.status != TicketStatus::Pending {
        card.expires_at = None;
        card.blocks.retain(|b| {
            !matches!(b, Block::InputFields { .. } | Block::ChoiceOptions { .. })
                && !matches!(b, Block::KvRows { id, .. } if id == ABOUT)
        });
    }
    if ticket.status == TicketStatus::NeedsDesktop {
        card.actions.clear();
        card.blocks.push(notice(
            "desktop",
            "Continue on the computer",
            NoticeTone::Attention,
        ));
    } else if ticket.status != TicketStatus::Pending {
        card.actions.clear();
        let continuing = ticket.status == TicketStatus::Resolved
            && ticket.deferral == TicketDeferral::Approval
            && approved(ticket)
            && !failed(ticket.resolution.as_ref());
        let (status, summary) = if continuing {
            (CardStatus::Pending, "Approved · continuing")
        } else if ticket.status == TicketStatus::Expired {
            (CardStatus::Expired, "Request expired")
        } else if ticket.status == TicketStatus::Rejected {
            (CardStatus::Failed, "Request cancelled")
        } else if failed(ticket.resolution.as_ref()) {
            (CardStatus::Failed, "The step could not be completed")
        } else {
            receipt(ticket)
        };
        card.status = status;
        card.collapsed_summary = summary.into();
        card.blocks.push(notice(
            "receipt",
            summary,
            if continuing {
                NoticeTone::Working
            } else {
                NoticeTone::Neutral
            },
        ));
        if ticket
            .resolution
            .as_ref()
            .is_some_and(|v| v.get("saved") == Some(&Value::Bool(true)))
        {
            card.blocks
                .push(notice("saved", "Saved to vault", NoticeTone::Neutral));
        }
        let filled = ticket.status == TicketStatus::Resolved
            && ticket
                .resolution
                .as_ref()
                .and_then(|v| v.get("status"))
                .and_then(Value::as_str)
                == Some("filled");
        if let Some(form) = ticket.form.as_ref().filter(|_| filled) {
            let labels = form
                .fields
                .iter()
                .map(|f| f.label.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            card.blocks.push(rows(
                "filled",
                vec![
                    row("Fields filled", labels),
                    row(
                        "Page",
                        "Not submitted. It's waiting in the agent's browser tab for you to check.",
                    ),
                    row(
                        "To submit",
                        "Ask the agent to submit it. You'll be asked to approve before anything is sent.",
                    ),
                ],
            ));
            card.actions.push(action(
                "ask-submit",
                "Ask the agent to submit",
                ActionTarget::FollowUp {
                    text: if form.payment {
                        "Submit the payment form you just filled in.".into()
                    } else {
                        "Submit the form you just filled in.".into()
                    },
                },
                false,
            ));
        }
    }
    card
}

fn team_card(id: String, title: String, status: CardStatus) -> Card {
    Card {
        id,
        kind: CardKind::Handoff,
        site: CardSite {
            origin: None,
            name: "Team".into(),
            favicon_ref: None,
        },
        collapsed_summary: title.clone(),
        title,
        subtitle: None,
        status,
        blocks: vec![],
        actions: vec![],
        interaction: CardInteraction::None,
        ticket_id: None,
        expires_at: None,
    }
}

fn team_person(id: &str, name: &str) -> CardPerson {
    CardPerson {
        agent_id: Some(id.into()),
        name: name.into(),
        initials: name.chars().take(2).collect(),
        color: None,
        role: None,
    }
}

pub(crate) fn enrich(kind: &mut EventKind, state: &SessionState) {
    enrich_at(kind, state, state.last_seq + 1);
}

pub(crate) fn enrich_at(kind: &mut EventKind, state: &SessionState, seq: u64) {
    match kind {
        EventKind::TeamHired {
            agent_id,
            name,
            ticket_id,
            request_message_id,
            card,
        } if card.is_none() => {
            let provenance = ticket_id
                .as_deref()
                .or_else(|| request_message_id.as_ref().map(|id| id.as_str()))
                .unwrap_or(agent_id);
            let mut projected = team_card(
                format!("hired:{agent_id}:{provenance}"),
                format!("{name} joined your team"),
                CardStatus::Done,
            );
            projected.blocks.push(Block::People {
                id: "teammate".into(),
                from: None,
                to: team_person(agent_id, name),
            });
            projected.actions.push(action(
                "open",
                "Open conversation",
                ActionTarget::OpenThread {
                    agent_id: agent_id.clone(),
                    anchor: None,
                },
                false,
            ));
            if request_message_id.is_some() && ticket_id.is_none() {
                projected.actions.push(action(
                    "undo",
                    "Undo hire",
                    ActionTarget::TeamCommand {
                        agent_id: agent_id.clone(),
                        command: TeamCardCommand::UndoHire,
                    },
                    false,
                ));
            }
            let mut retire = action(
                "retire",
                "Retire",
                ActionTarget::TeamCommand {
                    agent_id: agent_id.clone(),
                    command: TeamCardCommand::Retire,
                },
                false,
            );
            retire.style = ActionStyle::Destructive;
            projected.actions.push(retire);
            *card = Some(projected);
        }
        EventKind::TeamHandoff {
            handoff_id,
            from_agent,
            to_agent,
            goal,
            card,
            ..
        } if card.is_none() => {
            let mut projected = team_card(
                format!("handoff:{handoff_id}"),
                goal.clone(),
                CardStatus::Pending,
            );
            projected.blocks.push(Block::People {
                id: "handoff".into(),
                from: Some(team_person(from_agent, from_agent)),
                to: team_person(to_agent, to_agent),
            });
            projected.actions.push(action(
                "open",
                "Open conversation",
                ActionTarget::OpenThread {
                    agent_id: to_agent.clone(),
                    anchor: Some(format!("handoff:{handoff_id}")),
                },
                false,
            ));
            *card = Some(projected);
        }
        EventKind::TeamHandoffEnded {
            handoff_id,
            status,
            summary,
            card,
        } if card.is_none() => {
            let id = format!("handoff:{handoff_id}");
            let label = match status {
                HandoffStatus::Open => "Work in progress",
                HandoffStatus::Done => "Work completed",
                HandoffStatus::Failed => "Work failed",
                HandoffStatus::Cancelled => "Work cancelled",
            };
            let mut projected = state
                .cards
                .get(&id)
                .cloned()
                .unwrap_or_else(|| team_card(id, label.into(), CardStatus::Pending));
            projected.status = match status {
                HandoffStatus::Open => CardStatus::Pending,
                HandoffStatus::Done => CardStatus::Done,
                _ => CardStatus::Failed,
            };
            projected.collapsed_summary = label.into();
            projected
                .blocks
                .retain(|block| !matches!(block, Block::Notice { id, .. } if id == "outcome"));
            projected.blocks.push(notice(
                "outcome",
                summary.as_deref().unwrap_or(label),
                NoticeTone::Neutral,
            ));
            *card = Some(projected);
        }
        EventKind::TeamQuestion {
            question_id,
            from_agent,
            text,
            options,
            card,
        } if card.is_none() => {
            let mut projected = team_card(
                format!("question:{question_id}"),
                text.clone(),
                CardStatus::Pending,
            );
            projected.blocks.push(Block::People {
                id: "question".into(),
                from: Some(team_person(from_agent, from_agent)),
                to: team_person("chief", "Chief"),
            });
            if !options.is_empty() {
                projected
                    .blocks
                    .push(notice("options", options.join(" · "), NoticeTone::Info));
            }
            *card = Some(projected);
        }
        EventKind::TeamAnswered { question_id, card } if card.is_none() => {
            let id = format!("question:{question_id}");
            let mut projected = state
                .cards
                .get(&id)
                .cloned()
                .unwrap_or_else(|| team_card(id, "Question answered".into(), CardStatus::Done));
            projected.status = CardStatus::Done;
            projected.collapsed_summary = "Question answered".into();
            *card = Some(projected);
        }
        EventKind::TeamTransferred {
            from_agent,
            to_agent,
            routine_ids,
            responsibilities,
            card,
        } if card.is_none() => {
            let mut projected = team_card(
                format!("transfer:{from_agent}:{to_agent}:{seq}"),
                "Responsibilities transferred".into(),
                CardStatus::Done,
            );
            projected.blocks.push(Block::People {
                id: "transfer".into(),
                from: Some(team_person(from_agent, from_agent)),
                to: team_person(to_agent, to_agent),
            });
            projected.blocks.push(notice(
                "details",
                format!(
                    "{} routines · {}",
                    routine_ids.len(),
                    responsibilities.join("; ")
                ),
                NoticeTone::Neutral,
            ));
            *card = Some(projected);
        }
        EventKind::TeamRetired {
            agent_id,
            successor,
            card,
        } if card.is_none() => {
            let mut projected = team_card(
                format!("teammate:{agent_id}"),
                "Teammate retired".into(),
                CardStatus::Done,
            );
            projected.blocks.push(Block::People {
                id: "retired".into(),
                from: None,
                to: team_person(agent_id, agent_id),
            });
            if let Some(id) = successor {
                projected.blocks.push(notice(
                    "successor",
                    format!("Work transferred to {id}"),
                    NoticeTone::Neutral,
                ));
            }
            *card = Some(projected);
        }
        EventKind::TeamRestored { agent_id, card } if card.is_none() => {
            let mut projected = team_card(
                format!("teammate:{agent_id}"),
                "Teammate restored".into(),
                CardStatus::Done,
            );
            projected.blocks.push(Block::People {
                id: "restored".into(),
                from: None,
                to: team_person(agent_id, agent_id),
            });
            projected.actions.push(action(
                "open",
                "Open conversation",
                ActionTarget::OpenThread {
                    agent_id: agent_id.clone(),
                    anchor: None,
                },
                false,
            ));
            *card = Some(projected);
        }
        EventKind::TicketOpened { ticket: source } => {
            if source.card.is_none() {
                source.card = Some(ticket(source));
            }
        }
        EventKind::TicketResolved {
            ticket_id,
            resolution,
            card,
        } => {
            if card.is_none() {
                if let Some(source) = state.tickets.get(ticket_id) {
                    let mut source = source.clone();
                    source.status = TicketStatus::Resolved;
                    source.resolution = Some(resolution.clone());
                    *card = Some(ticket(&source));
                }
            }
        }
        EventKind::TicketRejected {
            ticket_id,
            reason,
            card,
        } => {
            if card.is_none() {
                if let Some(source) = state.tickets.get(ticket_id) {
                    let mut source = source.clone();
                    source.status = TicketStatus::Rejected;
                    source.resolution = Some(serde_json::json!({"reason":reason}));
                    *card = Some(ticket(&source));
                }
            }
        }
        EventKind::TicketExpired { ticket_id, card } => {
            if card.is_none() {
                if let Some(source) = state.tickets.get(ticket_id) {
                    let mut source = source.clone();
                    source.status = TicketStatus::Expired;
                    *card = Some(ticket(&source));
                }
            }
        }
        EventKind::ToolEnded {
            tool_call_id,
            result,
            is_error,
            card,
        } if card.is_none() && result.get("status").and_then(Value::as_str) != Some("deferred") => {
            if let Some(source) = state.tickets.values().find(|t| {
                t.tool_call_id == *tool_call_id
                    && t.deferral == TicketDeferral::Approval
                    && t.status == TicketStatus::Resolved
                    && approved(t)
            }) {
                let mut projected = ticket(source);
                projected.kind = CardKind::Activity;
                projected.interaction = CardInteraction::None;
                projected.status = if *is_error || failed(Some(result)) {
                    CardStatus::Failed
                } else {
                    CardStatus::Done
                };
                projected.collapsed_summary = if projected.status == CardStatus::Failed {
                    "Approved operation failed"
                } else {
                    "Approved operation completed"
                }
                .into();
                projected.blocks = vec![
                    notice("decision", "Approved by you", NoticeTone::Neutral),
                    notice("outcome", &projected.collapsed_summary, NoticeTone::Neutral),
                ];
                *card = Some(projected);
            }
        }
        _ => {}
    }
}

pub(crate) async fn interrupt_approvals(
    shared: &crate::engine::Shared,
    id: &SessionId,
) -> crate::persistence::Result<()> {
    let _gate = shared.ticket_gate.lock().await;
    let state = shared.db.state(id).await?;
    let mut pending: std::collections::HashSet<String> = state
        .tickets
        .values()
        .filter(|t| {
            t.deferral == TicketDeferral::Approval
                && t.status == TicketStatus::Resolved
                && approved(t)
        })
        .map(|t| t.tool_call_id.clone())
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let mut after = 0;
    loop {
        let events = shared.db.events(id, after).await?;
        if events.is_empty() {
            break;
        }
        for event in events {
            after = event.seq;
            if let EventKind::ToolEnded {
                tool_call_id,
                card: Some(card),
                ..
            } = event.kind
            {
                if card.kind == CardKind::Activity && card.status != CardStatus::Pending {
                    pending.remove(&tool_call_id);
                }
            }
        }
    }
    for tool_call_id in pending {
        shared
            .emit(
                id,
                None,
                EventKind::ToolEnded {
                    tool_call_id,
                    result: serde_json::json!({"status":"interrupted"}),
                    is_error: true,
                    card: None,
                },
            )
            .await?;
    }
    Ok(())
}
