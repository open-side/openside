//! Presentation-only conversation contracts. Ticket and policy state remain authoritative.
use crate::{LoginProvider, TicketId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Canonical Card wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Card {
    #[doc = "Stable identity within its card namespace; never an authorization token."]
    pub id: String,
    #[doc = "Semantic presentation family, not permission to act."]
    pub kind: CardKind,
    #[doc = "Source attribution with an exact origin boundary."]
    pub site: CardSite,
    #[doc = "Plain-text heading, bounded to 160 Unicode scalar values."]
    pub title: String,
    #[doc = "Optional plain-text context, bounded to 280 Unicode scalar values."]
    pub subtitle: Option<String>,
    #[doc = "Presentation lifecycle only; fresh ticket state governs actions."]
    pub status: CardStatus,
    #[doc = "Ordered typed content; no HTML or executable templates."]
    pub blocks: Vec<Block>,
    #[doc = "Typed intents validated against the authoritative source."]
    pub actions: Vec<Action>,
    #[doc = "Interactive cards require a daemon-issued ticket."]
    pub interaction: CardInteraction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[doc = "Daemon-issued authority reference; forbidden in model presentations."]
    pub ticket_id: Option<TicketId>,
    #[doc = "Value-free receipt or pending summary, bounded to 240 characters."]
    pub collapsed_summary: String,
}

/// Canonical CardKind wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CardKind {
    #[doc = "Typed ProductList presentation variant; source authorization remains unchanged."]
    ProductList,
    #[doc = "Typed Table presentation variant; source authorization remains unchanged."]
    Table,
    #[doc = "Typed PlaceList presentation variant; source authorization remains unchanged."]
    PlaceList,
    #[doc = "Typed OrderSummary presentation variant; source authorization remains unchanged."]
    OrderSummary,
    #[doc = "Typed Payment presentation variant; source authorization remains unchanged."]
    Payment,
    #[doc = "Typed SecureLogin presentation variant; source authorization remains unchanged."]
    SecureLogin,
    #[doc = "Typed LoginChoice presentation variant; source authorization remains unchanged."]
    LoginChoice,
    #[doc = "Typed Otp presentation variant; source authorization remains unchanged."]
    Otp,
    #[doc = "Typed PaymentCard presentation variant; source authorization remains unchanged."]
    PaymentCard,
    #[doc = "Typed Confirmation presentation variant; source authorization remains unchanged."]
    Confirmation,
    #[doc = "Typed Form presentation variant; source authorization remains unchanged."]
    Form,
    #[doc = "Typed ConnectAccount presentation variant; source authorization remains unchanged."]
    ConnectAccount,
    #[doc = "Typed Activity presentation variant; source authorization remains unchanged."]
    Activity,
    #[doc = "Typed Handoff presentation variant; source authorization remains unchanged."]
    Handoff,
    #[doc = "Typed Hire presentation variant; source authorization remains unchanged."]
    Hire,
    #[doc = "Typed SecretInput presentation variant; source authorization remains unchanged."]
    SecretInput,
    #[doc = "Typed Takeover presentation variant; source authorization remains unchanged."]
    Takeover,
}
/// Canonical CardStatus wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CardStatus {
    #[doc = "Typed Pending presentation variant; source authorization remains unchanged."]
    Pending,
    #[doc = "Typed Done presentation variant; source authorization remains unchanged."]
    Done,
    #[doc = "Typed Failed presentation variant; source authorization remains unchanged."]
    Failed,
    #[doc = "Typed Expired presentation variant; source authorization remains unchanged."]
    Expired,
}
/// Canonical CardInteraction wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CardInteraction {
    #[doc = "Typed None presentation variant; source authorization remains unchanged."]
    None,
    #[doc = "Typed Ask presentation variant; source authorization remains unchanged."]
    Ask,
}

/// Canonical CardSite wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardSite {
    // None only for local/non-site work (e.g. hire, handoff, filesystem activity).
    #[doc = "Exact canonical URL origin, including nondefault port."]
    pub origin: Option<String>,
    #[doc = "Plain-text display name, not a verified identity claim."]
    pub name: String,
    #[doc = "Daemon-normalized icon reference; never a remote image source."]
    pub favicon_ref: Option<FaviconRef>,
}
/// Canonical FaviconRef wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum FaviconRef {
    #[doc = "Typed Origin presentation variant; source authorization remains unchanged."]
    Origin {
        #[doc = "Exact canonical URL origin, including nondefault port."]
        origin: String,
    },
    #[doc = "Typed Data presentation variant; source authorization remains unchanged."]
    Data {
        #[doc = "Validated static PNG, at most 32 pixels per side and 8 KiB."]
        data_url: String,
    },
}
/// Canonical MediaRef wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaRef {
    #[doc = "Session-relative raster artifact; remote URLs and traversal are forbidden."]
    pub path: String,
} // session-relative daemon artifact
/// Canonical Money wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Money {
    #[doc = "Signed integer amount in currency minor units, without floating point."]
    pub minor_units: i64,
    #[doc = "Upper-case ISO currency code."]
    pub currency: String,
}
/// Canonical Rating wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    #[doc = "Rating in thousandths, between zero and milli_max."]
    pub milli_value: u32,
    #[doc = "Positive rating scale in thousandths, at most 10000."]
    pub milli_max: u32,
    #[doc = "Optional public rating count."]
    pub count: Option<u32>,
}
/// Canonical CardLink wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardLink {
    #[doc = "Validated HTTP(S) navigation URL without credentials or secret material."]
    pub url: String,
    #[doc = "Plain-text accessible label; never markup."]
    pub label: String,
}
/// Canonical CardValue wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CardValue {
    #[doc = "Typed Text presentation variant; source authorization remains unchanged."]
    Text {
        #[doc = "Plain text rendered without interpreting markup or instructions."]
        text: String,
    },
    #[doc = "Typed Money presentation variant; source authorization remains unchanged."]
    Money {
        #[doc = "Captured amount, not an authorization or computed charge."]
        money: Money,
    },
    #[doc = "Typed Link presentation variant; source authorization remains unchanged."]
    Link {
        #[doc = "Explicit navigation only; never background fetching."]
        link: CardLink,
    },
    #[doc = "Typed Rating presentation variant; source authorization remains unchanged."]
    Rating {
        #[doc = "Bounded integer rating metadata."]
        rating: Rating,
    },
    #[doc = "Typed Image presentation variant; source authorization remains unchanged."]
    Image {
        #[doc = "Daemon-materialized raster artifact in the current session."]
        image: MediaRef,
        #[doc = "Safe text retained when raster materialization fails."]
        alt: String,
    },
}
/// Canonical KvRow wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KvRow {
    #[doc = "Plain-text accessible label; never markup."]
    pub label: String,
    #[doc = "Typed display value, never a transient secret answer."]
    pub value: CardValue,
}
/// Canonical ListItem wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListItem {
    #[doc = "Stable identity within its card namespace; never an authorization token."]
    pub id: String,
    #[doc = "Plain-text display name, not a verified identity claim."]
    pub name: String,
    #[doc = "At most eight plain-text facts per item."]
    pub facts: Vec<String>,
    #[doc = "Daemon-materialized raster artifact in the current session."]
    pub image: Option<MediaRef>,
    #[doc = "Safe fallback text independent of successful image loading."]
    pub image_alt: Option<String>,
    #[doc = "Captured price; not an authorization to purchase."]
    pub price: Option<Money>,
    #[doc = "Bounded integer rating metadata."]
    pub rating: Option<Rating>,
    #[doc = "Explicit navigation only; never background fetching."]
    pub link: Option<CardLink>,
    #[doc = "Optional item count between 1 and 9999."]
    pub quantity: Option<u32>,
    #[doc = "Source attribution with an exact origin boundary."]
    pub site: Option<CardSite>,
}
/// Canonical TableColumn wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableColumn {
    #[doc = "Stable identity within its card namespace; never an authorization token."]
    pub id: String,
    #[doc = "Plain-text accessible label; never markup."]
    pub label: String,
}
/// Canonical TableLayout wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum TableLayout {
    #[doc = "Typed Grid presentation variant; source authorization remains unchanged."]
    Grid,
    #[doc = "Typed Scroll presentation variant; source authorization remains unchanged."]
    Scroll,
}
/// Canonical ChoiceLayout wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ChoiceLayout {
    #[doc = "Typed Stack presentation variant; source authorization remains unchanged."]
    Stack,
    #[doc = "Typed Providers presentation variant; source authorization remains unchanged."]
    Providers,
}
/// Canonical ChoiceOption wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    #[doc = "Stable identity within its card namespace; never an authorization token."]
    pub id: String,
    #[doc = "Plain-text accessible label; never markup."]
    pub label: String,
    #[doc = "Plain-text secondary context bounded to 280 characters."]
    pub description: Option<String>,
    #[doc = "Recognized provider selects a bundled glyph, not a remote image."]
    pub provider: Option<LoginProvider>,
    // An immediate page-provider/account action; None = radio selection for submit.
    #[doc = "Optional immediate authoritative action; absence means selection for submit."]
    pub action: Option<Action>,
}
/// Canonical InputKind wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum InputKind {
    #[doc = "Typed Text presentation variant; source authorization remains unchanged."]
    Text,
    #[doc = "Typed Email presentation variant; source authorization remains unchanged."]
    Email,
    #[doc = "Typed Tel presentation variant; source authorization remains unchanged."]
    Tel,
    #[doc = "Typed Password presentation variant; source authorization remains unchanged."]
    Password,
    #[doc = "Typed Checkbox presentation variant; source authorization remains unchanged."]
    Checkbox,
    #[doc = "Typed Textarea presentation variant; source authorization remains unchanged."]
    Textarea,
}
/// Canonical CardField wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CardField {
    #[doc = "Typed Name presentation variant; source authorization remains unchanged."]
    Name,
    #[doc = "Typed Number presentation variant; source authorization remains unchanged."]
    Number,
    #[doc = "Typed Expiry presentation variant; source authorization remains unchanged."]
    Expiry,
    #[doc = "Typed Cvc presentation variant; source authorization remains unchanged."]
    Cvc,
    #[doc = "Typed PostalCode presentation variant; source authorization remains unchanged."]
    PostalCode,
}
/// Canonical InputValidation wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum InputValidation {
    #[doc = "Typed Text presentation variant; source authorization remains unchanged."]
    Text,
    #[doc = "Typed Email presentation variant; source authorization remains unchanged."]
    Email,
    #[doc = "Typed Phone presentation variant; source authorization remains unchanged."]
    Phone,
    #[doc = "Typed Otp presentation variant; source authorization remains unchanged."]
    Otp,
    #[doc = "Typed CardNumber presentation variant; source authorization remains unchanged."]
    CardNumber,
    #[doc = "Typed CardExpiry presentation variant; source authorization remains unchanged."]
    CardExpiry,
    #[doc = "Typed CardCvc presentation variant; source authorization remains unchanged."]
    CardCvc,
    #[doc = "Typed PostalCode presentation variant; source authorization remains unchanged."]
    PostalCode,
}
/// Canonical FieldInitial wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum FieldInitial {
    #[doc = "Typed Text presentation variant; source authorization remains unchanged."]
    Text(String),
    #[doc = "Typed Checked presentation variant; source authorization remains unchanged."]
    Checked(bool),
}
/// Canonical InputField wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputField {
    #[doc = "Stable identity within its card namespace; never an authorization token."]
    pub id: String,
    #[doc = "Plain-text accessible label; never markup."]
    pub label: String,
    #[doc = "Semantic presentation family, not permission to act."]
    pub kind: InputKind,
    #[doc = "Transient answer must never reach model, transcript, or logs."]
    pub secret: bool,
    #[doc = "Requires secret handling and bounded site-channel validation."]
    pub otp: bool,
    #[doc = "Daemon-owned payment semantic; every payment field is secret."]
    pub card: Option<CardField>,
    #[doc = "Empty input is rejected only when this flag is true."]
    pub required: bool,
    #[doc = "Standard browser autocomplete metadata, not executable rules."]
    pub autocomplete: Option<String>,
    #[doc = "Nonsecret plain-text hint; never a saved answer."]
    pub placeholder: Option<String>,
    #[doc = "Fixed daemon validation vocabulary; no caller-defined regex."]
    pub validation: InputValidation,
    #[doc = "Maximum accepted characters, at most 4096 with tighter semantic limits."]
    pub max_length: u16,
    #[schemars(extend("x-person-only" = null))]
    #[doc = "Nonsecret initial value; prohibited for secret, OTP, and payment fields."]
    pub initial: Option<FieldInitial>, // prohibited for any secret/otp/card field
    #[schemars(extend("x-person-only" = []))]
    #[doc = "Explicit nonsecret suggestions only, never derived from a transient answer."]
    pub suggestions: Vec<String>, // explicit non-secret suggestions only
    #[doc = "Verified exact destination frame origin, not merely the merchant origin."]
    pub target_origin: Option<String>, // verified origin; needed for embedded frame fields
}
/// Canonical NoticeTone wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum NoticeTone {
    #[doc = "Typed Neutral presentation variant; source authorization remains unchanged."]
    Neutral,
    #[doc = "Typed Info presentation variant; source authorization remains unchanged."]
    Info,
    #[doc = "Typed Working presentation variant; source authorization remains unchanged."]
    Working,
    #[doc = "Typed Success presentation variant; source authorization remains unchanged."]
    Success,
    #[doc = "Typed Attention presentation variant; source authorization remains unchanged."]
    Attention,
    #[doc = "Typed Danger presentation variant; source authorization remains unchanged."]
    Danger,
}
/// Canonical NoticePlacement wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum NoticePlacement {
    #[doc = "Typed Inline presentation variant; source authorization remains unchanged."]
    Inline,
    #[doc = "Typed Bubble presentation variant; source authorization remains unchanged."]
    Bubble,
}
/// Canonical CardPerson wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardPerson {
    #[doc = "Daemon-derived roster identity; not model-selected authority."]
    pub agent_id: Option<String>,
    #[doc = "Plain-text display name, not a verified identity claim."]
    pub name: String,
    #[doc = "Bounded text avatar fallback."]
    pub initials: String,
    #[doc = "Allowlisted team avatar token, never arbitrary CSS."]
    pub color: Option<String>,
    #[doc = "Plain-text roster responsibility."]
    pub role: Option<String>,
}
/// Canonical Block wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum Block {
    #[doc = "Typed KvRows presentation variant; source authorization remains unchanged."]
    KvRows {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Ordered data rows; tables require exactly the column count."]
        rows: Vec<KvRow>,
        #[doc = "Optional captured ledger total; not calculated by the renderer."]
        total: Option<KvRow>,
    },
    #[doc = "Typed ListItems presentation variant; source authorization remains unchanged."]
    ListItems {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Stable list identities, at most 30 across a card."]
        items: Vec<ListItem>,
    },
    #[doc = "Typed Table presentation variant; source authorization remains unchanged."]
    Table {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Stable typed table columns, at most twelve."]
        columns: Vec<TableColumn>,
        #[doc = "Ordered data rows; tables require exactly the column count."]
        rows: Vec<Vec<CardValue>>,
        #[doc = "Presentation layout only, not a merchant-specific component."]
        layout: TableLayout,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[doc = "Optional source marks exactly parallel to table rows."]
        row_sites: Option<Vec<Option<CardSite>>>,
    },
    /// Human-facing choices can contain saved-account identities; not agent observations.
    ChoiceOptions {
        /// Stable block identity.
        id: String,
        /// Plain-text question label.
        label: Option<String>,
        /// Person-only choices, including saved-account identity and provider actions.
        #[schemars(extend("x-person-only" = []))]
        options: Vec<ChoiceOption>,
        /// Layout does not grant authority.
        layout: ChoiceLayout,
        /// Whether the person must choose before submitting.
        required: bool,
    },
    #[doc = "Typed InputFields presentation variant; source authorization remains unchanged."]
    InputFields {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Input metadata only; transient answers are never serialized here."]
        fields: Vec<InputField>,
    },
    #[doc = "Typed Notice presentation variant; source authorization remains unchanged."]
    Notice {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Presentation emphasis, not an authorization badge."]
        tone: NoticeTone,
        #[doc = "Plain text rendered without interpreting markup or instructions."]
        text: String,
        #[doc = "At most one trailing bubble per card."]
        placement: NoticePlacement,
    },
    #[doc = "Typed Media presentation variant; source authorization remains unchanged."]
    Media {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Session-owned bounded raster artifact."]
        media: MediaRef,
        #[doc = "Safe text retained when raster materialization fails."]
        alt: String,
        #[doc = "Optional safe plain-text image context."]
        caption: Option<String>,
    },
    #[doc = "Typed People presentation variant; source authorization remains unchanged."]
    People {
        #[doc = "Stable identity within its card namespace; never an authorization token."]
        id: String,
        #[doc = "Optional daemon-derived source participant."]
        from: Option<CardPerson>,
        #[doc = "Daemon-derived recipient or uncreated hire preview."]
        to: CardPerson,
    },
}

/// Canonical Action wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Action {
    #[doc = "Stable identity within its card namespace; never an authorization token."]
    pub id: String,
    #[doc = "Plain-text accessible label; never markup."]
    pub label: String,
    #[doc = "Visual recommendation; at most one primary per action area."]
    pub style: ActionStyle,
    #[doc = "Typed dispatch intent; appearance grants no authority."]
    pub target: ActionTarget,
}
/// Canonical ActionStyle wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ActionStyle {
    #[doc = "Typed Primary presentation variant; source authorization remains unchanged."]
    Primary,
    #[doc = "Typed Secondary presentation variant; source authorization remains unchanged."]
    Secondary,
    #[doc = "Typed Destructive presentation variant; source authorization remains unchanged."]
    Destructive,
}
/// Canonical ChoiceAnswerEncoding wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ChoiceAnswerEncoding {
    #[doc = "Typed PageOption presentation variant; source authorization remains unchanged."]
    PageOption,
    #[doc = "Typed InfoAnswer presentation variant; source authorization remains unchanged."]
    InfoAnswer,
}
/// Canonical FieldAnswerEncoding wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum FieldAnswerEncoding {
    #[doc = "Typed Login presentation variant; source authorization remains unchanged."]
    Login,
    #[doc = "Typed Form presentation variant; source authorization remains unchanged."]
    Form,
    #[doc = "Typed Scalar presentation variant; source authorization remains unchanged."]
    Scalar,
}
/// Canonical TicketAnswer wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum TicketAnswer {
    #[doc = "Typed Confirm presentation variant; source authorization remains unchanged."]
    Confirm {
        #[doc = "Decision on the exact captured operation, not a success claim."]
        approved: bool,
    },
    #[doc = "Typed Information presentation variant; source authorization remains unchanged."]
    Information {
        #[doc = "Reference to an allowed field in this card."]
        field_id: String,
    },
    #[doc = "Typed SelectedChoice presentation variant; source authorization remains unchanged."]
    SelectedChoice {
        #[doc = "Reference to an allowed choice block in this card."]
        block_id: String,
        #[doc = "Existing authoritative ticket wire encoding."]
        encoding: ChoiceAnswerEncoding,
    },
    #[doc = "Typed Fields presentation variant; source authorization remains unchanged."]
    Fields {
        #[doc = "References to allowed field blocks in this card."]
        block_ids: Vec<String>,
        #[doc = "Existing authoritative ticket wire encoding."]
        encoding: FieldAnswerEncoding,
        #[doc = "Optional explicit save-consent checkbox identity."]
        save_field_id: Option<String>,
    },
    #[doc = "Typed LoginAccount presentation variant; source authorization remains unchanged."]
    LoginAccount {
        #[doc = "Vault account identity already offered by the ticket."]
        item_id: String,
    },
    #[doc = "Typed LoginAlternative presentation variant; source authorization remains unchanged."]
    LoginAlternative {
        #[doc = "Pinned page alternative already offered by the ticket."]
        alternative_id: String,
    },
    #[doc = "Typed OtpCode presentation variant; source authorization remains unchanged."]
    OtpCode {
        #[doc = "Reference to an allowed field in this card."]
        field_id: String,
    },
    #[doc = "Typed OtpAuthenticator presentation variant; source authorization remains unchanged."]
    OtpAuthenticator {
        #[doc = "Reference to an allowed field in this card."]
        field_id: String,
        #[doc = "Optional explicit save-consent checkbox identity."]
        save_field_id: String,
    },
    /// Decision and edited draft are governed by the team contract, not card text.
    Hire {
        #[doc = "Team decision literal: hire, not_now, or never."]
        decision: String,
        #[doc = "Optional nonsecret edit block governed by the team contract."]
        edited_block_id: Option<String>,
    },
}
/// Canonical ActionTarget wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ActionTarget {
    #[doc = "Typed TicketResolve presentation variant; source authorization remains unchanged."]
    TicketResolve {
        #[doc = "Typed answer mapping validated against the current ticket."]
        answer: TicketAnswer,
    },
    #[doc = "Typed TicketReject presentation variant; source authorization remains unchanged."]
    TicketReject,
    #[doc = "Typed TicketDone presentation variant; source authorization remains unchanged."]
    TicketDone,
    #[doc = "Typed FollowUp presentation variant; source authorization remains unchanged."]
    FollowUp {
        #[doc = "Plain text rendered without interpreting markup or instructions."]
        text: String,
    },
    #[doc = "Typed Link presentation variant; source authorization remains unchanged."]
    Link {
        #[doc = "Validated HTTP(S) navigation URL without credentials or secret material."]
        url: String,
    },
    #[doc = "Typed OpenThread presentation variant; source authorization remains unchanged."]
    OpenThread {
        #[doc = "Daemon-derived roster identity; not model-selected authority."]
        agent_id: String,
        #[doc = "Optional local thread anchor, never a remote URL."]
        anchor: Option<String>,
    },
    #[doc = "Typed OpenTicket presentation variant; source authorization remains unchanged."]
    OpenTicket {
        #[doc = "Daemon-issued authority reference; forbidden in model presentations."]
        ticket_id: TicketId,
    },
    #[doc = "Typed OpenConnection presentation variant; source authorization remains unchanged."]
    OpenConnection {
        #[doc = "Daemon-derived connection identity; existing access scope still applies."]
        connection_id: String,
    },
    #[doc = "Typed TeamCommand presentation variant; source authorization remains unchanged."]
    TeamCommand {
        #[doc = "Daemon-derived roster identity; not model-selected authority."]
        agent_id: String,
        #[doc = "Fresh team operation subject to existing authorization."]
        command: TeamCardCommand,
    },
    #[doc = "Typed Takeover presentation variant; source authorization remains unchanged."]
    Takeover,
}
/// Canonical TeamCardCommand wire contract; presentation never grants authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum TeamCardCommand {
    #[doc = "Typed UndoHire presentation variant; source authorization remains unchanged."]
    UndoHire,
    #[doc = "Typed Retire presentation variant; source authorization remains unchanged."]
    Retire,
}

/// Canonical PresentRequest wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentRequest {
    #[doc = "Daemon-owned payment semantic; every payment field is secret."]
    pub card: Card,
    #[doc = "Transport-only image sources; never persisted as remote references."]
    pub images: Vec<PresentImage>,
}
/// Canonical PresentImage wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentImage {
    #[doc = "Unique image alias matching ASCII letters, digits, underscore, or hyphen."]
    pub key: String,
    #[doc = "Validated HTTP(S) navigation URL without credentials or secret material."]
    pub url: String,
}
/// Canonical PresentResult wire contract; presentation never grants authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentResult {
    #[doc = "Daemon-assigned presentation identity returned without duplicating the card."]
    pub card_id: String,
}

/// Verified page form metadata; values are transient and never part of this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketForm {
    /// Exact verified top-document origin, not a registrable domain.
    #[doc = "Exact canonical URL origin, including nondefault port."]
    pub origin: String,
    /// Untrusted page display name; not an identity attestation.
    #[doc = "Value-free metadata governed by the containing contract."]
    pub site_name: String,
    /// Metadata in page order, with exact destination origins for embedded frames.
    #[doc = "Input metadata only; transient answers are never serialized here."]
    pub fields: Vec<InputField>,
    /// Requires recognized card autocomplete fields and secret handling for every value.
    #[doc = "Value-free metadata governed by the containing contract."]
    pub payment: bool,
    /// Daemon-owned fill-only action label; never authorizes submission.
    #[doc = "Value-free metadata governed by the containing contract."]
    pub submit_label: String,
}
