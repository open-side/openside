use std::io::Cursor as Input;

use openside_proto::{
    Action, ActionStyle, ActionTarget, Block, Card, CardKind, CardStatus, CardValue, FieldInitial,
    InputKind, InputValidation, Money, TeamCardCommand, TicketForm,
};
use serde_json::{json, Value};

use super::{event, json_lines, ticket, Fixture};
use crate::{
    client::Client,
    ui::{
        cards::{self, Context},
        ColorWhen, Env, Terminal, Ui,
    },
};

fn fixtures() -> Vec<Card> {
    serde_json::from_str(include_str!(
        "../../../openside-proto/src/cards_fixtures.json"
    ))
    .unwrap()
}

fn fixture(kind: CardKind) -> Card {
    fixtures()
        .into_iter()
        .find(|card| card.kind == kind)
        .unwrap()
}

fn context() -> Context<'static> {
    Context {
        session_id: "s-1",
        web_origin: Some("https://local.example"),
        client: None,
    }
}

fn render(card: &Card, width: usize) -> String {
    cards::render(&Ui::fixed(false, false, width), card, &context())
}

#[test]
fn all_kinds_and_lifecycle_have_plain_non_tty_no_color_and_narrow_snapshots() {
    let headings = [
        "Wireless earbuds",
        "Plan comparison",
        "Lunch nearby",
        "Order summary",
        "Place this order?",
        "Log in",
        "Choose an account",
        "Verification code",
        "Add card details",
        "Send this message?",
        "Fill guest details",
        "Connect work.example",
        "Looking for earbuds",
        "Handed to Mina",
        "A teammate for price checks",
        "Enter the secret",
        "Finish this step in the browser",
    ];
    let fixtures = fixtures();
    assert_eq!(fixtures.len(), headings.len());
    let no_color = Ui::detect(
        ColorWhen::Auto,
        &Env {
            no_color: true,
            utf8: false,
            ..Env::default()
        },
        &Terminal {
            stdout_tty: true,
            stderr_tty: true,
            width: Some(80),
        },
    )
    .unwrap();
    let non_tty = Ui::detect(ColorWhen::Auto, &Env::default(), &Terminal::default());
    assert!(non_tty.is_none());
    for (card, heading) in fixtures.into_iter().zip(headings) {
        for (status, label) in [
            (CardStatus::Pending, "pending"),
            (CardStatus::Done, "done"),
            (CardStatus::Failed, "failed"),
            (CardStatus::Expired, "expired"),
        ] {
            let mut card = card.clone();
            card.status = status;
            let plain = render(&card, 80);
            assert_eq!(
                plain.lines().next().unwrap(),
                format!("{heading} [{label}]")
            );
            assert_eq!(cards::render(&no_color, &card, &context()), plain);
            assert_eq!(cards::render(&cards::plain_ui(), &card, &context()), plain);
            assert!(!plain.contains('\x1b'));
            let narrow = render(&card, 40);
            assert!(
                narrow.lines().all(|line| line.chars().count() <= 40),
                "{narrow}"
            );
            assert!(narrow.contains(label));
            if status != CardStatus::Pending {
                assert!(!plain.contains("ticket resolve"), "{plain}");
                assert!(!plain.contains("ticket reject"), "{plain}");
                assert!(!plain.contains("ticket done"), "{plain}");
            }
        }
    }
}

#[test]
fn list_ledger_media_and_rating_snapshots_preserve_captured_values() {
    let products = render(&fixture(CardKind::ProductList), 80);
    assert_eq!(products,
        "Wireless earbuds [done]\nshop.example\nOrigin: https://shop.example\nThree options\n  1. Everyday Buds\n    Noise cancellation\n    24-hour case\n    USD 79.00\n    4.4/5 (231 ratings)\n    View earbuds: https://shop.example/products/buds\n    Image: Black earbuds and case [cards/media/buds.webp]\n  Compare these: send this follow-up: Compare the earbuds you found.\n");
    let order = render(&fixture(CardKind::OrderSummary), 80);
    assert!(order.contains("Shipping: USD 5.00\n"), "{order}");
    assert!(order.contains("Total: USD 24.31\n"), "{order}");
    let mut card = fixture(CardKind::Form);
    card.blocks = vec![
        Block::Media {
            id: "photo".into(),
            media: openside_proto::MediaRef {
                path: "cards/media/photo.png".into(),
            },
            alt: "Entrance photo".into(),
            caption: Some("The north door".into()),
        },
        Block::KvRows {
            id: "amounts".into(),
            rows: vec![openside_proto::KvRow {
                label: "Yen".into(),
                value: CardValue::Money {
                    money: Money {
                        minor_units: -123,
                        currency: "JPY".into(),
                    },
                },
            }],
            total: Some(openside_proto::KvRow {
                label: "Dinars".into(),
                value: CardValue::Money {
                    money: Money {
                        minor_units: 1234,
                        currency: "KWD".into(),
                    },
                },
            }),
        },
    ];
    card.actions.clear();
    let rendered = render(&card, 80);
    assert!(rendered.contains("Image: Entrance photo [cards/media/photo.png]\n  The north door\n"));
    assert!(rendered.contains("Yen: JPY -123\n  Dinars: KWD 1.234\n"));
}

#[test]
fn narrow_tables_wrap_without_truncating_cells_or_source_origins() {
    let mut card = fixture(CardKind::Table);
    card.blocks = vec![Block::Table {
        id: "table".into(),
        columns: vec![
            openside_proto::TableColumn {
                id: "name".into(),
                label: "Plan".into(),
            },
            openside_proto::TableColumn {
                id: "cost".into(),
                label: "Price".into(),
            },
        ],
        rows: vec![vec![
            CardValue::Text {
                text: "A long comparison plan".into(),
            },
            CardValue::Money {
                money: Money {
                    minor_units: 1234,
                    currency: "USD".into(),
                },
            },
        ]],
        layout: openside_proto::TableLayout::Grid,
        row_sites: None,
    }];
    assert_eq!(render(&card, 40), "Plan comparison [done]\nComparison\n  Plan                    Price\n  A long comparison plan  USD 12.34\n");
    if let Block::Table {
        rows, row_sites, ..
    } = &mut card.blocks[0]
    {
        rows[0][0] = CardValue::Text {
            text: "Long source-specific plan retains every word and finalcanary".into(),
        };
        *row_sites = Some(vec![Some(openside_proto::CardSite {
            name: "Vendor".into(),
            origin: Some("https://vendor.example:8443".into()),
            favicon_ref: None,
        })]);
    }
    let text = render(&card, 40);
    assert!(!text.contains("..."));
    assert!(text.lines().all(|line| line.chars().count() <= 40));
    let joined = text.split_whitespace().collect::<String>();
    assert!(joined.contains("finalcanary"), "{text}");
    if let Block::Table { rows, .. } = &mut card.blocks[0] {
        rows[0] = vec![
            CardValue::Text { text: "A".into() },
            CardValue::Text { text: "B".into() },
        ];
    }
    let source = render(&card, 40).split_whitespace().collect::<String>();
    assert!(source.contains("https://vendor.example:8443"), "{source}");
}

#[test]
fn untrusted_text_cannot_emit_control_sequences_or_private_initials() {
    let mut card = fixture(CardKind::PaymentCard);
    card.title = "Payment\x1b[31m\r\n\u{009b}31m".into();
    card.site.name = "Shop\x07".into();
    card.site.origin = Some("https://shop.example:8443\x1b]52;c;payload\x07".into());
    for block in &mut card.blocks {
        if let Block::InputFields { fields, .. } = block {
            for field in fields {
                field.label.push_str("\x1b[2J");
                field.initial = Some(FieldInitial::Text("PRIVATE-INITIAL-CANARY".into()));
                field.placeholder = Some("PRIVATE-PLACEHOLDER-CANARY".into());
                field.suggestions = vec!["PRIVATE-SUGGESTION-CANARY".into()];
            }
        }
    }
    let text = render(&card, 40);
    assert!(!text.contains("PRIVATE-"));
    assert!(text.chars().all(|c| c == '\n' || !c.is_control()));
    assert!(text.contains("[hidden input]"));
    assert!(!text.contains("sent") && !text.contains("paid"));
}

#[test]
fn hire_handoff_and_undo_have_truthful_web_and_team_equivalents() {
    let hire = render(&fixture(CardKind::Hire), 200);
    assert!(hire.contains("Open web: https://local.example/sessions/s-1?ticket=hire1"));
    assert!(hire.contains("CLI: openside team proposals"));
    assert!(hire.contains("openside ticket resolve hire1 hire"));
    let handoff = render(&fixture(CardKind::Handoff), 200);
    assert!(handoff.contains("https://local.example/agents/mina#handoff%3Ah1"));
    assert!(handoff.contains("openside team show mina"));
    let mut hired = fixture(CardKind::Hire);
    hired.status = CardStatus::Done;
    hired.actions = vec![Action {
        id: "undo".into(),
        label: "Undo".into(),
        style: ActionStyle::Secondary,
        target: ActionTarget::TeamCommand {
            agent_id: "mina".into(),
            command: TeamCardCommand::UndoHire,
        },
    }];
    assert!(render(&hired, 200).contains("Undo: openside team undo mina"));
    let unknown = Context {
        session_id: "s-1",
        web_origin: None,
        client: None,
    };
    assert!(!cards::render(&Ui::fixed(false, false, 200), &hired, &unknown).contains("Open web:"));
}

fn page_ticket(payment: bool) -> Value {
    let mut value = ticket();
    let mut card = fixture(if payment {
        CardKind::PaymentCard
    } else {
        CardKind::Form
    });
    card.id = "ticket:t-1".into();
    card.ticket_id = Some(openside_proto::TicketId::new("t-1"));
    let fields = card
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::InputFields { fields, .. } => Some(fields.clone()),
            _ => None,
        })
        .unwrap();
    value["kind"] = json!("info");
    value["deferral"] = json!("external");
    value["summary"] = json!("LEGACY-RAW-SUMMARY");
    value["form"] = serde_json::to_value(TicketForm {
        origin: card.site.origin.clone().unwrap(),
        site_name: card.site.name.clone(),
        fields,
        payment,
        submit_label: "Fill fields".into(),
    })
    .unwrap();
    value["card"] = serde_json::to_value(card).unwrap();
    value
}

#[tokio::test]
async fn form_metadata_prompts_and_payment_pipe_keep_all_canaries_out_of_output() {
    let fixture = Fixture::new().await;
    fixture.state.lock().ticket = Some(page_ticket(false));
    let mut input = Input::new("Guest Name\n");
    let (code, output, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1"], &mut input)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":{"values":{"party":"Guest Name"}}})
    );
    assert!(!output.contains("Guest Name") && !error.contains("Guest Name"));
    assert!(output.contains("[done]") && output.contains("Fields filled"));
    let values = [
        "PAYMENT-NAME-CANARY",
        "4111111111111111",
        "12/30",
        "987",
        "POSTAL-CANARY",
    ];
    fixture.state.lock().ticket = Some(page_ticket(true));
    for arguments in [
        &["--json", "ticket", "resolve", "t-1", "-"][..],
        &["ticket", "resolve", "t-1"][..],
    ] {
        let mut input = Input::new(format!("{}\n", values.join("\n")));
        let (code, output, error) = fixture.run_with_input(arguments, &mut input).await;
        assert_eq!(code, 0, "{error}");
        for secret in values {
            assert!(
                !output.contains(secret) && !error.contains(secret),
                "{output}\n{error}"
            );
        }
        assert_eq!(
            fixture.state.lock().requests.last().unwrap().body,
            json!({"value":{"values":{"name":values[0],"number":values[1],"expiry":values[2],"cvc":values[3],"postal":values[4]}}})
        );
    }
    fixture.state.lock().secret_error = true;
    let mut input = Input::new(format!("{}\n", values.join("\n")));
    let (code, output, error) = fixture
        .run_with_input(&["--json", "ticket", "resolve", "t-1", "-"], &mut input)
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("details withheld"));
    for secret in values {
        assert!(!output.contains(secret) && !error.contains(secret));
    }
}

#[tokio::test]
async fn private_form_secret_otp_and_login_values_are_refused_in_argv() {
    let fixture = Fixture::new().await;
    let mut cases = vec![page_ticket(true)];
    for kind in ["secret_input", "login"] {
        let mut ticket = ticket();
        ticket["kind"] = json!(kind);
        cases.push(ticket);
    }
    let mut otp = page_ticket(false);
    otp["form"]["fields"][0]["otp"] = json!(true);
    cases.push(otp);
    let mut secret = page_ticket(false);
    secret["form"]["fields"][0]["secret"] = json!(true);
    cases.push(secret);
    for ticket in cases {
        fixture.state.lock().ticket = Some(ticket);
        let mut input = Input::new("untouched");
        let (code, output, error) = fixture
            .run_with_input(
                &["ticket", "resolve", "t-1", "ARGV-PRIVATE-CANARY"],
                &mut input,
            )
            .await;
        assert_eq!(code, 1);
        assert!(error.contains("hidden prompt or stdin"));
        assert!(!output.contains("ARGV-PRIVATE-CANARY") && !error.contains("ARGV-PRIVATE-CANARY"));
        assert_eq!(input.position(), 0);
        assert_eq!(fixture.state.lock().requests.last().unwrap().method, "GET");
    }
}

#[test]
fn real_form_checkboxes_initials_destinations_and_errors_use_metadata() {
    let value = page_ticket(false);
    let mut form: TicketForm = serde_json::from_value(value["form"].clone()).unwrap();
    form.fields[0].initial = Some(FieldInitial::Text("Original guest".into()));
    let mut checkbox = form.fields[0].clone();
    checkbox.id = "consent".into();
    checkbox.label = "Consent".into();
    checkbox.kind = InputKind::Checkbox;
    checkbox.initial = None;
    checkbox.target_origin = Some("https://frame.example:8443".into());
    form.fields.push(checkbox);
    let mut prompts = Vec::new();
    assert_eq!(
        crate::prompt::form_answer(&form, &mut Input::new("\nyes\n"), &mut prompts).unwrap(),
        json!({"values":{"party":"Original guest","consent":true}})
    );
    assert!(String::from_utf8(prompts)
        .unwrap()
        .contains("https://frame.example:8443"));
    assert!(
        crate::prompt::form_answer(&form, &mut Input::new("X\nno\n"), &mut Vec::new())
            .unwrap_err()
            .contains("required")
    );
    form.fields[0].max_length = 2;
    let error = crate::prompt::form_answer(
        &form,
        &mut Input::new("LENGTH-CANARY\nyes\n"),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(error.contains("maximum length") && !error.contains("LENGTH-CANARY"));
    let mut payment = form.clone();
    payment.payment = true;
    payment.fields[0].secret = false;
    payment.fields[0].initial = Some(FieldInitial::Text("SECRET-INITIAL-CANARY".into()));
    let mut stderr = Vec::new();
    let _ = crate::prompt::form_answer(&payment, &mut Input::new("ok\nyes\n"), &mut stderr);
    assert!(!String::from_utf8(stderr)
        .unwrap()
        .contains("SECRET-INITIAL-CANARY"));
    assert!(cards::sensitive(&openside_proto::InputField {
        validation: InputValidation::Otp,
        ..form.fields[0].clone()
    }));
}

async fn replay(fixture: &Fixture, events: Vec<Value>) {
    let mut state = fixture.state.lock();
    // The SSE events arrive after the running-session snapshot, including its terminal marker.
    state.session = Some(
        json!({"id":"s-1","title":"test","model":"fake","mode":"guard","workspace":null,"status":"running","last_seq":0,"queue":[]}),
    );
    state.events = events;
}

#[tokio::test]
async fn pending_ticket_is_rendered_from_card_once_and_json_preserves_canonical_events() {
    let fixture = Fixture::new().await;
    let ticket = page_ticket(false);
    let mut card = ticket["card"].clone();
    card["title"] = json!("Canonical guest request");
    let mut ticket = ticket;
    ticket["card"] = card.clone();
    fixture.state.lock().ticket = Some(ticket.clone());
    let events = vec![
        event(
            1,
            "tool.started",
            json!({"tool_call_id":"call-1","name":"request_form","args":{"tab_id":"tab"}}),
        ),
        event(2, "ticket.opened", json!({"ticket":ticket})),
        event(3, "session.settled", json!({})),
    ];
    replay(&fixture, events.clone()).await;
    let (code, output, error) = fixture
        .run(&["session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(error.matches("Canonical guest request").count(), 1);
    assert!(!error.contains("LEGACY-RAW-SUMMARY") && !output.contains("LEGACY-RAW-SUMMARY"));
    assert!(!error.contains("Wants to:") && !error.contains("Resolve in another process"));
    replay(&fixture, events.clone()).await;
    let (code, output, error) = fixture
        .run(&["--json", "session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    let lines = json_lines(&output);
    assert_eq!(&lines[..events.len()], events.as_slice());
    assert_eq!(lines.len(), events.len() + 1);
    assert!(error.is_empty(), "{error}");
}

#[tokio::test]
async fn presented_team_and_lifecycle_events_render_without_handcrafted_json() {
    let product = serde_json::to_value(fixture(CardKind::ProductList)).unwrap();
    let handoff = serde_json::to_value(fixture(CardKind::Handoff)).unwrap();
    let fixture = Fixture::new().await;
    let mut receipt = handoff.clone();
    receipt["status"] = json!("failed");
    receipt["title"] = json!("Work could not finish");
    let events = vec![
        event(
            1,
            "card.presented",
            json!({"card":product,"tool_call_id":"present1"}),
        ),
        event(
            2,
            "team.handoff",
            json!({"handoff_id":"h1","from_agent":"chief","to_agent":"mina","handoff_kind":"task","goal":"Price checks","dedupe_key":"price","card":handoff}),
        ),
        event(
            3,
            "team.handoff_ended",
            json!({"handoff_id":"h1","status":"failed","summary":"Work could not finish","card":receipt}),
        ),
        event(4, "session.settled", json!({})),
    ];
    replay(&fixture, events.clone()).await;
    let (code, output, error) = fixture
        .run(&["session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    assert!(
        output.contains("Wireless earbuds [done]")
            && output.contains("Handed to Mina [pending]")
            && output.contains("Work could not finish [failed]")
    );
    assert!(
        output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("team show mina"),
        "{output}"
    );
    replay(&fixture, events.clone()).await;
    let (code, output, error) = fixture
        .run(&["--json", "session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(&json_lines(&output)[..events.len()], events.as_slice());
}

#[test]
fn http_web_origin_is_exact_and_a_custom_socket_never_guesses_one() {
    let client = Client::new(None, Some("http://127.0.0.1:49123"), Some("token")).unwrap();
    assert_eq!(
        client.web_origin().as_deref(),
        Some("http://127.0.0.1:49123")
    );
    assert_eq!(
        client.team_command("undo", Some("mina")),
        "openside --url http://127.0.0.1:49123 team undo mina"
    );
    assert_eq!(
        client.ticket_command("t-1", "resolve", Some("-")),
        "openside --url http://127.0.0.1:49123 ticket resolve t-1 -"
    );
    assert_eq!(
        client.team_command("proposals", None),
        "openside --url http://127.0.0.1:49123 team proposals"
    );
    let client = Client::new(
        Some(std::path::Path::new("/tmp/isolated-card-test.sock")),
        None,
        None,
    )
    .unwrap();
    assert!(client.web_origin().is_none());
    assert_eq!(
        client.team_command("show", Some("Mina's thread")),
        "openside --socket /tmp/isolated-card-test.sock team show 'Mina'\\''s thread'"
    );
}

#[tokio::test]
async fn live_cards_replace_tool_notices_and_show_approval_continuing_before_outcome() {
    let source = page_ticket(false);
    let mut continuing = source["card"].clone();
    continuing["title"] = json!("Approved operation");
    continuing["status"] = json!("pending");
    continuing["actions"] = json!([]);
    continuing["blocks"] = json!([{"type":"notice","id":"receipt","text":"Approved; continuing","tone":"working","placement":"inline"}]);
    let mut completed = continuing.clone();
    completed["status"] = json!("done");
    completed["blocks"][0]["text"] = json!("Operation completed");
    let events = [
        event(
            1,
            "tool.started",
            json!({"tool_call_id":"call-1","name":"request_form","args":{}}),
        ),
        event(2, "ticket.opened", json!({"ticket":source})),
        event(
            3,
            "ticket.resolved",
            json!({"ticket_id":"t-1","resolution":{"approved":true},"card":continuing}),
        ),
        event(
            4,
            "tool.ended",
            json!({"tool_call_id":"call-1","result":{"status":"filled"},"is_error":false,"card":completed}),
        ),
    ];
    let mut live = crate::ui::live::Live::new(Ui::fixed(false, false, 40));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    {
        let mut err = crate::ui::live::Stderr::new(&mut stderr);
        for event in events {
            let event: openside_proto::Event = serde_json::from_value(event).unwrap();
            if let Some(card) = cards::event_card(&event.kind) {
                live.card(card, &context(), &mut stdout, &mut err).unwrap();
            }
            live.event(&event, &mut stdout, &mut err).unwrap();
        }
    }
    let output = String::from_utf8(stdout).unwrap();
    let error = String::from_utf8(stderr).unwrap();
    assert!(output.is_empty());
    assert_eq!(error.matches("Fill guest details").count(), 1);
    assert!(error.contains("Approved operation [pending]"));
    assert!(error.contains("Approved operation [done]"));
    assert!(
        !error.contains("LEGACY-RAW-SUMMARY")
            && !error.contains("Answered")
            && !error.contains("Using request")
    );
    assert!(error.lines().all(|line| line.chars().count() <= 40));
    let colored = cards::render(
        &Ui::fixed(true, true, 40),
        &fixture(CardKind::Table),
        &context(),
    );
    assert!(colored.contains("\x1b["));
}

#[tokio::test]
async fn ticket_show_and_list_render_only_canonical_card_content() {
    let fixture = Fixture::new().await;
    fixture.state.lock().ticket = Some(page_ticket(false));
    for command in [&["ticket", "show", "t-1"][..], &["ticket", "list"][..]] {
        let (code, output, error) = fixture.run(command).await;
        assert_eq!(code, 0, "{error}");
        assert_eq!(output.matches("Fill guest details").count(), 1, "{output}");
        assert!(!output.contains("LEGACY-RAW-SUMMARY") && !output.contains("tool:"));
    }
}

#[tokio::test]
async fn run_wait_prints_presented_and_hired_cards_and_json_keeps_their_event_envelopes() {
    let product = serde_json::to_value(fixture(CardKind::ProductList)).unwrap();
    let mut hired = fixture(CardKind::Hire);
    hired.status = CardStatus::Done;
    hired.title = "Mina joined your team".into();
    hired.ticket_id = None;
    hired.blocks = vec![Block::People {
        id: "teammate".into(),
        from: None,
        to: openside_proto::CardPerson {
            agent_id: Some("mina".into()),
            name: "Mina".into(),
            initials: "M".into(),
            color: None,
            role: None,
        },
    }];
    hired.actions.clear();
    let fixture = Fixture::new().await;
    let source = vec![
        event(1, "message.injected", json!({"message_id":"m-1"})),
        event(
            2,
            "card.presented",
            json!({"card":product,"tool_call_id":"present1"}),
        ),
        event(
            3,
            "team.hired",
            json!({"agent_id":"mina","name":"Mina","ticket_id":null,"request_message_id":"m-1","card":hired}),
        ),
        event(
            4,
            "message.completed",
            json!({"message_id":"m-1","outcome":"completed"}),
        ),
    ];
    for json_mode in [false, true] {
        replay(&fixture, source.clone()).await;
        let args = if json_mode {
            vec!["--json", "run", "compare", "--session", "s-1", "--wait"]
        } else {
            vec!["run", "compare", "--session", "s-1", "--wait"]
        };
        let (code, output, error) = fixture.run(&args).await;
        assert_eq!(code, 0, "{error}");
        if json_mode {
            let lines = json_lines(&output);
            assert_eq!(&lines[..source.len()], source.as_slice());
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line["kind"] == "card.presented")
                    .count(),
                1
            );
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line["kind"] == "team.hired")
                    .count(),
                1
            );
        } else {
            assert_eq!(output.matches("Wireless earbuds [done]").count(), 1);
            assert_eq!(output.matches("Mina joined your team [done]").count(), 1);
            assert!(
                output
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains("team show mina"),
                "{output}"
            );
        }
    }
}
