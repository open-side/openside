use openside_proto::{
    Action, ActionTarget, Block, Card, CardKind, CardStatus, CardValue, EventKind, FieldInitial,
    InputField, InputKind, InputValidation, Money, Rating, TeamCardCommand, TicketAnswer,
};

use super::{
    clean,
    table::{Cell, Column, Table},
    text::{visible_width, wrap_spans},
    Span, Ui,
};
use crate::{
    client::{encode_segment, Client},
    plain,
};

pub(crate) struct Context<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) web_origin: Option<&'a str>,
    pub(crate) client: Option<&'a Client>,
}

pub(crate) fn event_card(kind: &EventKind) -> Option<&Card> {
    match kind {
        EventKind::CardPresented { card, .. } => Some(card),
        EventKind::TicketOpened { ticket } => ticket.card.as_ref(),
        EventKind::TicketResolved { card, .. }
        | EventKind::TicketRejected { card, .. }
        | EventKind::TicketExpired { card, .. }
        | EventKind::ToolEnded { card, .. }
        | EventKind::TeamHired { card, .. }
        | EventKind::TeamHandoff { card, .. }
        | EventKind::TeamHandoffEnded { card, .. } => card.as_ref(),
        _ => None,
    }
}

pub(crate) fn plain_ui() -> Ui {
    Ui {
        color: false,
        err_color: false,
        unicode: false,
        width: 80,
        live: false,
    }
}

pub(crate) fn sensitive(field: &InputField) -> bool {
    field.secret
        || field.otp
        || field.card.is_some()
        || field.kind == InputKind::Password
        || matches!(
            field.validation,
            InputValidation::Otp
                | InputValidation::CardNumber
                | InputValidation::CardExpiry
                | InputValidation::CardCvc
        )
}

fn line(out: &mut String, ui: &Ui, indent: usize, text: &str) {
    let available = ui.width.saturating_sub(indent).max(1);
    for spans in wrap_spans(&[Span::plain(clean(text))], available, available) {
        out.push_str(&" ".repeat(indent));
        out.push_str(&ui.render(&spans));
        out.push('\n');
    }
}

fn decimal(value: u32) -> String {
    let mut text = format!("{}.{:03}", value / 1000, value % 1000);
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn rating(value: &Rating) -> String {
    let mut text = format!(
        "{}/{}",
        decimal(value.milli_value),
        decimal(value.milli_max)
    );
    if let Some(count) = value.count {
        text.push_str(&format!(" ({count} ratings)"));
    }
    text
}

fn money(value: &Money) -> String {
    let digits = match value.currency.as_str() {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "CLF" | "UYW" => 4,
        _ => 2,
    };
    let amount = value.minor_units.unsigned_abs();
    let sign = if value.minor_units < 0 { "-" } else { "" };
    let currency = clean(&value.currency);
    if digits == 0 {
        return format!("{currency} {sign}{amount}");
    }
    let scale = 10_u64.pow(digits);
    format!(
        "{currency} {sign}{}.{:0digits$}",
        amount / scale,
        amount % scale,
        digits = digits as usize
    )
}

fn value(value: &CardValue) -> String {
    match value {
        CardValue::Text { text } => clean(text),
        CardValue::Money { money: amount } => money(amount),
        CardValue::Link { link } => format!("{}: {}", clean(&link.label), clean(&link.url)),
        CardValue::Rating { rating: score } => rating(score),
        CardValue::Image { image, alt } => {
            format!("Image: {} [{}]", clean(alt), clean(&image.path))
        }
    }
}

fn web(context: &Context<'_>, path: &str) -> Option<String> {
    context
        .web_origin
        .map(|origin| format!("{}{path}", origin.trim_end_matches('/')))
}

fn ticket_command(context: &Context<'_>, id: &str, action: &str, value: Option<&str>) -> String {
    match context.client {
        Some(client) => client.ticket_command(id, action, value),
        None => {
            let mut command = format!("openside ticket {action} {}", quote(id));
            if let Some(value) = value {
                command.push(' ');
                command.push_str(&quote(value));
            }
            command
        }
    }
}

fn team_command(context: &Context<'_>, action: &str, id: Option<&str>) -> String {
    match context.client {
        Some(client) => client.team_command(action, id),
        None => format!(
            "openside team {action}{}",
            id.map(|id| format!(" {}", quote(id))).unwrap_or_default()
        ),
    }
}

fn quote(text: &str) -> String {
    let text = plain(text);
    if !text.is_empty()
        && text.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
    {
        text
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn ticket_web(context: &Context<'_>, id: &str) -> Option<String> {
    web(
        context,
        &format!(
            "/sessions/{}?ticket={}",
            encode_segment(context.session_id),
            encode_segment(id)
        ),
    )
}

fn action_hint(action: &Action, card: &Card, context: &Context<'_>) -> Option<String> {
    let ticket = card.ticket_id.as_ref().map(|id| id.as_str());
    let label = clean(&action.label);
    let hint = match &action.target {
        ActionTarget::Link { url } => format!("{label}: {}", clean(url)),
        ActionTarget::FollowUp { text } => format!("{label}: send this follow-up: {}", clean(text)),
        ActionTarget::OpenThread { agent_id, anchor } => {
            let path = format!(
                "/agents/{}{}",
                encode_segment(agent_id),
                anchor
                    .as_ref()
                    .map(|a| format!("#{}", encode_segment(a)))
                    .unwrap_or_default()
            );
            let cli = team_command(context, "show", Some(agent_id));
            match web(context, &path) {
                Some(url) => format!("{label}: Open web: {url}; CLI: {cli}"),
                None => format!("{label}: {cli}"),
            }
        }
        ActionTarget::OpenTicket { ticket_id } => {
            let cli = ticket_command(context, ticket_id.as_str(), "show", None);
            match ticket_web(context, ticket_id.as_str()) {
                Some(url) => format!("{label}: Open web: {url}; CLI: {cli}"),
                None => format!("{label}: {cli}"),
            }
        }
        ActionTarget::OpenConnection { .. } => {
            let url = web(context, "/connections")?;
            format!("{label}: Open web: {url}")
        }
        ActionTarget::TeamCommand { agent_id, command } => {
            let command = match command {
                TeamCardCommand::UndoHire => "undo",
                TeamCardCommand::Retire => "retire",
            };
            format!(
                "{label}: {}",
                team_command(context, command, Some(agent_id))
            )
        }
        ActionTarget::TicketResolve { answer } => {
            if card.status != CardStatus::Pending {
                return None;
            }
            let id = ticket?;
            let (arg, flags) = match answer {
                TicketAnswer::Confirm { approved } => (
                    Some(if *approved { "true" } else { "false" }.to_owned()),
                    None,
                ),
                TicketAnswer::Information { .. } => (None, None),
                TicketAnswer::SelectedChoice { .. } | TicketAnswer::Fields { .. } => (None, None),
                TicketAnswer::OtpCode { .. } => (Some("-".into()), None),
                TicketAnswer::OtpAuthenticator { .. } => {
                    (Some("-".into()), Some("--totp".to_owned()))
                }
                TicketAnswer::LoginAccount { item_id } => {
                    (None, Some(format!("--account {}", quote(item_id))))
                }
                TicketAnswer::LoginAlternative { alternative_id } => (
                    None,
                    Some(format!("--alternative {}", quote(alternative_id))),
                ),
                TicketAnswer::Hire { decision, .. } => (Some(decision.clone()), None),
            };
            let mut command = ticket_command(context, id, "resolve", arg.as_deref());
            if let Some(flags) = flags {
                command.push(' ');
                command.push_str(&flags);
            }
            format!("{label}: {command}")
        }
        ActionTarget::TicketReject => {
            if card.status != CardStatus::Pending {
                return None;
            }
            format!(
                "{label}: {}",
                ticket_command(context, ticket?, "reject", None)
            )
        }
        ActionTarget::TicketDone => {
            if card.status != CardStatus::Pending {
                return None;
            }
            format!(
                "{label}: {}",
                ticket_command(context, ticket?, "done", None)
            )
        }
        ActionTarget::Takeover => format!("{label}: Open web: {}", ticket_web(context, ticket?)?),
    };
    Some(hint)
}

pub(crate) fn render(ui: &Ui, card: &Card, context: &Context<'_>) -> String {
    let status = match card.status {
        CardStatus::Pending => "pending",
        CardStatus::Done => "done",
        CardStatus::Failed => "failed",
        CardStatus::Expired => "expired",
    };
    let mut out = String::new();
    line(&mut out, ui, 0, &format!("{} [{status}]", card.title));
    line(&mut out, ui, 0, &card.site.name);
    if let Some(origin) = &card.site.origin {
        line(&mut out, ui, 0, &format!("Origin: {origin}"));
    }
    if let Some(subtitle) = &card.subtitle {
        line(&mut out, ui, 0, subtitle);
    }
    for block in &card.blocks {
        match block {
            Block::KvRows { rows, total, .. } => {
                for row in rows {
                    line(
                        &mut out,
                        ui,
                        2,
                        &format!("{}: {}", row.label, value(&row.value)),
                    );
                }
                if let Some(total) = total {
                    line(
                        &mut out,
                        ui,
                        2,
                        &format!("{}: {}", total.label, value(&total.value)),
                    );
                }
            }
            Block::ListItems { items, .. } => {
                for (index, item) in items.iter().enumerate() {
                    line(
                        &mut out,
                        ui,
                        2,
                        &format!(
                            "{}. {}{}",
                            index + 1,
                            item.name,
                            item.quantity
                                .map(|q| format!(" (quantity {q})"))
                                .unwrap_or_default()
                        ),
                    );
                    if let Some(site) = &item.site {
                        line(&mut out, ui, 4, &site.name);
                        if let Some(origin) = &site.origin {
                            line(&mut out, ui, 4, &format!("Origin: {origin}"));
                        }
                    }
                    for fact in &item.facts {
                        line(&mut out, ui, 4, fact);
                    }
                    if let Some(price) = &item.price {
                        line(&mut out, ui, 4, &money(price));
                    }
                    if let Some(score) = &item.rating {
                        line(&mut out, ui, 4, &rating(score));
                    }
                    if let Some(link) = &item.link {
                        line(&mut out, ui, 4, &format!("{}: {}", link.label, link.url));
                    }
                    if let Some(image) = &item.image {
                        line(
                            &mut out,
                            ui,
                            4,
                            &format!(
                                "Image: {} [{}]",
                                item.image_alt.as_deref().unwrap_or(&item.name),
                                image.path
                            ),
                        );
                    }
                }
            }
            Block::Table {
                columns,
                rows,
                row_sites,
                ..
            } => {
                let has_sites = row_sites
                    .as_ref()
                    .is_some_and(|sites| sites.iter().any(Option::is_some));
                let mut headers: Vec<_> = columns
                    .iter()
                    .map(|column| Column::new(clean(&column.label)).flex(2))
                    .collect();
                if has_sites {
                    headers.push(Column::new("Source").flex(2));
                }
                let mut table = Table::new(headers);
                for (index, row) in rows.iter().enumerate() {
                    let mut cells: Vec<_> = row.iter().map(|v| Cell::text(value(v))).collect();
                    if has_sites {
                        let site = row_sites
                            .as_ref()
                            .and_then(|sites| sites.get(index))
                            .and_then(Option::as_ref);
                        cells.push(Cell::text(
                            site.map(|site| match &site.origin {
                                Some(origin) => format!("{} ({origin})", clean(&site.name)),
                                None => clean(&site.name),
                            })
                            .unwrap_or_default(),
                        ));
                    }
                    table.row(cells);
                }
                // Too many columns cannot each keep even one readable cell at phone width.
                if columns.len() + usize::from(has_sites) > ui.width.saturating_sub(2) / 4 {
                    for (index, row) in rows.iter().enumerate() {
                        line(&mut out, ui, 2, &format!("Row {}", index + 1));
                        for (column, cell) in columns.iter().zip(row) {
                            line(
                                &mut out,
                                ui,
                                4,
                                &format!("{}: {}", column.label, value(cell)),
                            );
                        }
                        if let Some(site) = row_sites
                            .as_ref()
                            .and_then(|sites| sites.get(index))
                            .and_then(Option::as_ref)
                        {
                            line(
                                &mut out,
                                ui,
                                4,
                                &format!(
                                    "Source: {}{}",
                                    site.name,
                                    site.origin
                                        .as_ref()
                                        .map(|o| format!(" ({o})"))
                                        .unwrap_or_default()
                                ),
                            );
                        }
                    }
                } else {
                    out.push_str(&table.render_wrapped(ui));
                }
            }
            Block::ChoiceOptions { label, options, .. } => {
                if let Some(label) = label {
                    line(&mut out, ui, 2, label);
                }
                for (index, option) in options.iter().enumerate() {
                    line(
                        &mut out,
                        ui,
                        2,
                        &format!("{}. {} [{}]", index + 1, option.label, option.id),
                    );
                    if let Some(description) = &option.description {
                        line(&mut out, ui, 4, description);
                    }
                    if let Some(action) = &option.action {
                        if let Some(hint) = action_hint(action, card, context) {
                            line(&mut out, ui, 4, &hint);
                        }
                    }
                }
            }
            Block::InputFields { fields, .. } => {
                for field in fields {
                    let hidden = sensitive(field) || card.kind == CardKind::PaymentCard;
                    let mut text = format!(
                        "{}{}{}",
                        field.label,
                        if field.required { " (required)" } else { "" },
                        if hidden { " [hidden input]" } else { "" }
                    );
                    if !hidden {
                        match &field.initial {
                            Some(FieldInitial::Text(initial)) => {
                                text.push_str(&format!(": {initial}"))
                            }
                            Some(FieldInitial::Checked(checked)) => {
                                text.push_str(if *checked { ": yes" } else { ": no" })
                            }
                            None => {}
                        }
                    }
                    line(&mut out, ui, 2, &text);
                    if let Some(origin) = &field.target_origin {
                        line(&mut out, ui, 4, &format!("Destination: {origin}"));
                    }
                    if !hidden {
                        if let Some(hint) = &field.placeholder {
                            line(&mut out, ui, 4, hint);
                        }
                        if !field.suggestions.is_empty() {
                            line(
                                &mut out,
                                ui,
                                4,
                                &format!("Suggestions: {}", field.suggestions.join(", ")),
                            );
                        }
                    }
                }
            }
            Block::Notice { text, .. } => line(&mut out, ui, 2, text),
            Block::Media {
                media,
                alt,
                caption,
                ..
            } => {
                line(&mut out, ui, 2, &format!("Image: {alt} [{}]", media.path));
                if let Some(caption) = caption {
                    line(&mut out, ui, 2, caption);
                }
            }
            Block::People { from, to, .. } => {
                let person = |p: &openside_proto::CardPerson| {
                    format!(
                        "{}{}",
                        p.name,
                        p.role
                            .as_ref()
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default()
                    )
                };
                let text = match from {
                    Some(from) => format!("{} -> {}", person(from), person(to)),
                    None => person(to),
                };
                line(&mut out, ui, 2, &text);
            }
        }
    }
    if card.blocks.is_empty() && card.collapsed_summary != card.title {
        line(&mut out, ui, 2, &card.collapsed_summary);
    }
    if matches!(card.kind, CardKind::Hire | CardKind::Handoff) {
        let path = match card.ticket_id.as_ref() {
            Some(id) => format!(
                "/sessions/{}?ticket={}",
                encode_segment(context.session_id),
                encode_segment(id.as_str())
            ),
            None => format!("/sessions/{}", encode_segment(context.session_id)),
        };
        if let Some(url) = web(context, &path) {
            line(&mut out, ui, 2, &format!("Open web: {url}"));
        }
        if !card
            .actions
            .iter()
            .any(|a| matches!(a.target, ActionTarget::OpenThread { .. }))
        {
            if let Some(id) = card.blocks.iter().find_map(|b| match b {
                Block::People { to, .. } => to.agent_id.as_deref(),
                _ => None,
            }) {
                line(
                    &mut out,
                    ui,
                    2,
                    &format!("CLI: {}", team_command(context, "show", Some(id))),
                );
            } else if card.kind == CardKind::Hire {
                line(
                    &mut out,
                    ui,
                    2,
                    &format!("CLI: {}", team_command(context, "proposals", None)),
                );
            }
        }
    }
    for action in &card.actions {
        if let Some(hint) = action_hint(action, card, context) {
            line(&mut out, ui, 2, &hint);
        }
    }
    if card.status == CardStatus::Pending
        && card.blocks.iter().any(|b| match b {
            Block::InputFields { fields, .. } => {
                fields.iter().any(sensitive) || card.kind == CardKind::PaymentCard
            }
            _ => false,
        })
    {
        line(&mut out, ui, 2, "Use the hidden prompt or pipe one line per field; never put private values in argv or shell history.");
    }
    if card.kind == CardKind::PaymentCard && card.status == CardStatus::Pending {
        line(
            &mut out,
            ui,
            2,
            "Fills fields only; no payment is submitted.",
        );
    }
    debug_assert!(out.lines().all(|line| visible_width(line) <= ui.width));
    out
}
