//! Permission policy engine.
//!
//! Requirements: D-1 (session modes), D-2 (Allow/Ask/Deny rules, Deny wins,
//! agent-level plus session-level layers), D-4 (sensitive actions always need a human),
//! D-5 (outbound allow-list, [`Egress`]), D-6 (no path returns vault secrets).
//!
//! The engine is a pure function of `(Policy, Action) -> Decision`. It performs no I/O.
//! Callers must pass **canonical** paths (symlinks resolved); canonicalisation and
//! symlink-escape checks belong to the sandbox crate.
//!
//! Precedence rules:
//! * Within one layer: `Deny > Ask > Allow`.
//! * Across layers: any `Deny` wins. Otherwise the topmost layer (session) that has a
//!   matching rule decides between `Ask` and `Allow`.
//! * Origin lists ([`OriginLists`]) only restrict: a denied origin, or an origin missing
//!   from a non-empty allow list, is `Deny` for every web capability. They never grant.
//!   Patterns and targets share one canonicalization ([`Origin`], [`OriginPattern`]).
//! * `Sensitive` actions (payment, post, message) are never auto-allowed. Rules may only
//!   tighten them to `Deny`.
//! * `VaultUse` is denied in `ReadOnly` mode because logging in changes remote state.

mod egress;
mod glob;
mod origin;

use std::path::{Component, Path, PathBuf};

pub use egress::{Egress, EgressError};
pub use glob::glob_match;
pub use origin::{Origin, OriginLists, OriginPattern};

/// Session permission mode (D-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Inspect browser and file context only.
    ReadOnly,
    /// Work inside approved folders; ask before anything else. The default.
    #[default]
    Guard,
    /// Read and write anywhere. Sensitive actions still need a human.
    FullAccess,
}

/// What an action does, independent of its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Read a file or directory. Target: absolute path.
    FsRead,
    /// Write, delete or rename a file. Target: absolute path.
    FsWrite,
    /// Run a command. Target: absolute working directory.
    Exec,
    /// Drive a browser page. Target: origin or URL.
    Browse,
    /// Network fetch outside the browser. Target: URL.
    Fetch,
    /// Use a saved credential through the vault. Target: origin.
    VaultUse,
    /// Payment, publishing a post, sending a message. Target: origin or description.
    Sensitive,
}

/// One requested action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Kind of action.
    pub capability: Capability,
    /// Path, origin or URL the action touches.
    pub target: String,
}

impl Action {
    /// Convenience constructor.
    pub fn new(capability: Capability, target: impl Into<String>) -> Self {
        Self {
            capability,
            target: target.into(),
        }
    }
}

/// Outcome of a rule or a decision. Ordered by severity: `Allow < Ask < Deny`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effect {
    /// Proceed without asking.
    Allow,
    /// Ask a human first.
    Ask,
    /// Block.
    Deny,
}

/// A rule: when `capability` matches and `pattern` matches the target, `effect` applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Effect when the rule matches.
    pub effect: Effect,
    /// Capability the rule applies to.
    pub capability: Capability,
    /// Glob over the action target (`*` within a segment, `**` across `/`).
    pub pattern: String,
}

impl Rule {
    /// Convenience constructor.
    pub fn new(effect: Effect, capability: Capability, pattern: impl Into<String>) -> Self {
        Self {
            effect,
            capability,
            pattern: pattern.into(),
        }
    }

    fn matches(&self, action: &Action) -> bool {
        self.capability == action.capability && glob_match(&self.pattern, &action.target)
    }
}

/// Rules belonging to one level (agent defaults or a single session).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleLayer {
    /// Rules in this layer. Order does not matter.
    pub rules: Vec<Rule>,
}

/// Why a decision came out the way it did (for audit logs and UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A `Deny` rule matched in the given layer (index 0 = lowest).
    DenyRule {
        /// Layer index of the matching rule.
        layer: usize,
    },
    /// The target origin matched [`OriginLists::deny`].
    OriginDenied,
    /// [`OriginLists::allow`] is non-empty and the target origin matched none of it.
    OriginNotAllowed,
    /// The topmost layer with a matching rule decided.
    Rule {
        /// Layer index that decided.
        layer: usize,
        /// Effect that rule produced.
        effect: Effect,
    },
    /// No rule matched; the session mode decided.
    Mode(Mode),
    /// Sensitive actions are never auto-allowed.
    SensitiveFloor,
}

/// Result of evaluating an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Final effect.
    pub effect: Effect,
    /// Why.
    pub reason: Reason,
}

/// A complete policy for one session.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Session permission mode.
    pub mode: Mode,
    /// Canonical absolute folders where Guard mode works without asking.
    pub approved_roots: Vec<PathBuf>,
    /// Rule layers ordered from lowest (agent defaults) to highest (session).
    pub layers: Vec<RuleLayer>,
    /// Per-origin restrictions, checked right after Deny rules.
    pub origins: OriginLists,
}

impl Policy {
    /// Evaluates `action`.
    pub fn decide(&self, action: &Action) -> Decision {
        // 1. Any matching Deny in any layer wins.
        for (i, layer) in self.layers.iter().enumerate() {
            if layer
                .rules
                .iter()
                .any(|r| r.effect == Effect::Deny && r.matches(action))
            {
                return Decision {
                    effect: Effect::Deny,
                    reason: Reason::DenyRule { layer: i },
                };
            }
        }

        // 2. Origin lists restrict web capabilities before anything can ask or allow.
        if let Some(reason) = self.origin_block(action) {
            return Decision {
                effect: Effect::Deny,
                reason,
            };
        }

        // 3. Sensitive actions are never auto-allowed.
        if action.capability == Capability::Sensitive {
            let base = if self.mode == Mode::ReadOnly {
                Effect::Deny
            } else {
                Effect::Ask
            };
            return Decision {
                effect: base,
                reason: Reason::SensitiveFloor,
            };
        }

        // 4. The topmost layer with a matching rule decides between Ask and Allow.
        for (i, layer) in self.layers.iter().enumerate().rev() {
            let mut best: Option<Effect> = None;
            for r in layer.rules.iter().filter(|r| r.matches(action)) {
                best = Some(best.map_or(r.effect, |b| b.max(r.effect)));
            }
            if let Some(effect) = best {
                // A rule cannot grant more than the mode ceiling allows for ReadOnly.
                if self.mode == Mode::ReadOnly
                    && mutates(action.capability)
                    && effect != Effect::Deny
                {
                    return Decision {
                        effect: Effect::Deny,
                        reason: Reason::Mode(Mode::ReadOnly),
                    };
                }
                return Decision {
                    effect,
                    reason: Reason::Rule { layer: i, effect },
                };
            }
        }

        // 5. Mode default.
        Decision {
            effect: self.mode_default(action),
            reason: Reason::Mode(self.mode),
        }
    }

    fn mode_default(&self, action: &Action) -> Effect {
        use Capability::*;
        match self.mode {
            Mode::ReadOnly => match action.capability {
                FsRead | Browse | Fetch => Effect::Allow,
                FsWrite | Exec | VaultUse => Effect::Deny,
                Sensitive => Effect::Deny,
            },
            Mode::Guard => match action.capability {
                Browse | Fetch | VaultUse => Effect::Allow,
                FsRead | FsWrite | Exec => {
                    if self.within_approved_roots(&action.target) {
                        Effect::Allow
                    } else {
                        Effect::Ask
                    }
                }
                Sensitive => Effect::Ask,
            },
            Mode::FullAccess => match action.capability {
                Sensitive => Effect::Ask,
                _ => Effect::Allow,
            },
        }
    }

    fn within_approved_roots(&self, target: &str) -> bool {
        self.approved_roots
            .iter()
            .any(|root| path_within(root, Path::new(target)))
    }

    fn origin_block(&self, action: &Action) -> Option<Reason> {
        use Capability::*;
        if !matches!(action.capability, Browse | Fetch | VaultUse | Sensitive) {
            return None;
        }
        self.origins.block(&Origin::parse(&action.target)?)
    }
}

/// Canonical `scheme://host[:port]` of an HTTP(S)/WS(S) URL target (see [`Origin`]), or
/// `None` for targets that are not such URLs (paths, free-text descriptions). Userinfo is
/// dropped so `https://good.example@evil.example` is judged as `https://evil.example`.
pub fn origin_of(target: &str) -> Option<String> {
    Origin::parse(target).map(|origin| origin.to_string())
}

/// Conservative substring heuristic for sensitive page targets (D-4: payment, purchase,
/// sending/posting a message, transfers, deletion), matched case-insensitively against an
/// accessible name or role. It is not intent detection: false positives only cost a
/// confirmation, and an unmatched label proves nothing about safety.
pub fn sensitive_label(name: &str, role: &str) -> bool {
    SENSITIVE_KEYWORDS.iter().any(|keyword| {
        [name, role].iter().any(|label| {
            label
                .as_bytes()
                .windows(keyword.len())
                .any(|value| value.eq_ignore_ascii_case(keyword.as_bytes()))
        })
    })
}

/// Keywords used by [`sensitive_label`]; documented in `docs/specs/policy.md`.
pub const SENSITIVE_KEYWORDS: &[&str] = &[
    "pay",
    "buy",
    "purchase",
    "checkout",
    "place order",
    "submit payment",
    "send",
    "transfer",
    "publish",
    "결제",
    "구매",
    "송금",
    "전송",
    "보내기",
    "게시",
    "delete",
    "삭제",
];

fn mutates(c: Capability) -> bool {
    matches!(
        c,
        Capability::FsWrite | Capability::Exec | Capability::VaultUse | Capability::Sensitive
    )
}

/// Lexical containment check: is `target` equal to or inside `root`?
///
/// Both paths must be absolute and canonical. Any `..` component in `target`, or a
/// relative `target`, is rejected (returns `false`) so a caller bug cannot widen access.
pub fn path_within(root: &Path, target: &Path) -> bool {
    if !root.is_absolute() || !target.is_absolute() {
        return false;
    }
    if target
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    target.starts_with(root)
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn guard(roots: &[&str], layers: Vec<RuleLayer>) -> Policy {
        Policy {
            mode: Mode::Guard,
            approved_roots: roots.iter().map(PathBuf::from).collect(),
            layers,
            origins: OriginLists::default(),
        }
    }

    const ALL: [Capability; 7] = [
        Capability::FsRead,
        Capability::FsWrite,
        Capability::Exec,
        Capability::Browse,
        Capability::Fetch,
        Capability::VaultUse,
        Capability::Sensitive,
    ];

    fn target(capability: Capability, inside: bool) -> &'static str {
        match (capability, inside) {
            (Capability::FsRead | Capability::FsWrite | Capability::Exec, true) => "/work/a",
            (Capability::FsRead | Capability::FsWrite | Capability::Exec, false) => "/elsewhere/a",
            _ => "https://site.example",
        }
    }

    /// D-1: every mode x capability x (inside/outside approved root) cell, with no rules.
    #[test]
    fn mode_capability_matrix_is_exhaustive() {
        use Capability::*;
        use Effect::*;
        let expected = |mode: Mode, capability: Capability, inside: bool| match mode {
            Mode::ReadOnly => match capability {
                FsRead | Browse | Fetch => Allow,
                FsWrite | Exec | VaultUse | Sensitive => Deny,
            },
            Mode::Guard => match capability {
                Browse | Fetch | VaultUse => Allow,
                FsRead | FsWrite | Exec if inside => Allow,
                FsRead | FsWrite | Exec | Sensitive => Ask,
            },
            Mode::FullAccess => match capability {
                Sensitive => Ask,
                _ => Allow,
            },
        };
        for mode in [Mode::ReadOnly, Mode::Guard, Mode::FullAccess] {
            let mut policy = guard(&["/work"], vec![]);
            policy.mode = mode;
            for capability in ALL {
                for inside in [true, false] {
                    let decision =
                        policy.decide(&Action::new(capability, target(capability, inside)));
                    assert_eq!(
                        decision.effect,
                        expected(mode, capability, inside),
                        "{mode:?} {capability:?} inside={inside}"
                    );
                }
            }
        }
    }

    #[test]
    fn origin_deny_list_blocks_every_web_capability_in_every_mode() {
        for mode in [Mode::ReadOnly, Mode::Guard, Mode::FullAccess] {
            let mut policy = guard(
                &[],
                vec![layer(vec![Rule::new(
                    Effect::Allow,
                    Capability::Browse,
                    "**",
                )])],
            );
            policy.mode = mode;
            policy.origins =
                OriginLists::new(Vec::<&str>::new(), ["https://*.evil.example"]).unwrap();
            for capability in [
                Capability::Browse,
                Capability::Fetch,
                Capability::VaultUse,
                Capability::Sensitive,
            ] {
                let decision =
                    policy.decide(&Action::new(capability, "https://cdn.evil.example/x?y=1"));
                assert_eq!(decision.effect, Effect::Deny, "{mode:?} {capability:?}");
                assert_eq!(decision.reason, Reason::OriginDenied);
            }
            assert_eq!(
                policy
                    .decide(&Action::new(Capability::Browse, "https://good.example"))
                    .effect,
                Effect::Allow
            );
        }
    }

    #[test]
    fn origin_allow_list_restricts_but_never_grants() {
        let mut policy = guard(&[], vec![]);
        policy.origins = OriginLists::new(
            ["https://docs.example", "http://127.0.0.1:*"],
            Vec::<&str>::new(),
        )
        .unwrap();
        let other = policy.decide(&Action::new(Capability::Browse, "https://other.example"));
        assert_eq!(
            (other.effect, other.reason),
            (Effect::Deny, Reason::OriginNotAllowed)
        );
        assert_eq!(
            policy
                .decide(&Action::new(
                    Capability::Browse,
                    "HTTPS://DOCS.example/path"
                ))
                .effect,
            Effect::Allow
        );
        assert_eq!(
            policy
                .decide(&Action::new(Capability::Browse, "http://127.0.0.1:4100"))
                .effect,
            Effect::Allow
        );
        // Allow-listed origins still hit the sensitive floor.
        assert_eq!(
            policy
                .decide(&Action::new(Capability::Sensitive, "https://docs.example"))
                .effect,
            Effect::Ask
        );
        // Userinfo cannot smuggle an allow-listed name in front of another host.
        assert_eq!(
            policy
                .decide(&Action::new(
                    Capability::Browse,
                    "https://docs.example@evil.example/"
                ))
                .effect,
            Effect::Deny
        );
        // Filesystem targets are not origins.
        assert_eq!(
            policy
                .decide(&Action::new(Capability::FsRead, "/tmp/x"))
                .reason,
            Reason::Mode(Mode::Guard)
        );
    }

    #[test]
    fn origin_of_normalizes_and_rejects_non_urls() {
        assert_eq!(
            origin_of("HTTP://Example.COM:80/a?b#c").as_deref(),
            Some("http://example.com")
        );
        assert_eq!(
            origin_of("https://u:p@host.example/").as_deref(),
            Some("https://host.example")
        );
        assert_eq!(origin_of("/etc/passwd"), None);
        assert_eq!(origin_of("pay now"), None);
        // Browsers skip extra slashes after a special scheme; so does the shared parser.
        assert_eq!(
            origin_of("https:///nohost").as_deref(),
            Some("https://nohost")
        );
        assert_eq!(origin_of("https:///"), None);
    }

    #[test]
    fn sensitive_labels_cover_payment_message_post_and_delete() {
        for keyword in SENSITIVE_KEYWORDS {
            assert!(sensitive_label(keyword, "button"));
            assert!(sensitive_label("Continue", keyword));
        }
        for label in [
            "Pay now",
            "Send money",
            "Transfer funds",
            "Publish post",
            "송금하기",
            "메시지 전송",
        ] {
            assert!(sensitive_label(label, "button"), "{label}");
        }
        assert!(!sensitive_label("Continue", "link"));
        assert!(!sensitive_label("Username", "textbox"));
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn capability() -> impl Strategy<Value = Capability> {
            prop::sample::select(ALL.to_vec())
        }
        fn effect() -> impl Strategy<Value = Effect> {
            prop::sample::select(vec![Effect::Allow, Effect::Ask, Effect::Deny])
        }
        fn mode() -> impl Strategy<Value = Mode> {
            prop::sample::select(vec![Mode::ReadOnly, Mode::Guard, Mode::FullAccess])
        }
        // Patterns either match the fixed targets or not, so rule matches are frequent.
        fn pattern() -> impl Strategy<Value = String> {
            prop::sample::select(vec![
                "**",
                "/work/**",
                "/elsewhere/**",
                "https://*",
                "nomatch",
            ])
            .prop_map(str::to_owned)
        }
        fn rule() -> impl Strategy<Value = Rule> {
            (effect(), capability(), pattern()).prop_map(|(e, c, p)| Rule::new(e, c, p))
        }
        fn layers() -> impl Strategy<Value = Vec<RuleLayer>> {
            prop::collection::vec(prop::collection::vec(rule(), 0..5), 0..4).prop_map(|layers| {
                layers
                    .into_iter()
                    .map(|rules| RuleLayer { rules })
                    .collect()
            })
        }
        fn action() -> impl Strategy<Value = Action> {
            (capability(), any::<bool>()).prop_map(|(c, inside)| Action::new(c, target(c, inside)))
        }
        fn policy(mode: Mode, layers: Vec<RuleLayer>) -> Policy {
            let mut p = guard(&["/work"], layers);
            p.mode = mode;
            p
        }

        proptest! {
            /// D-2: a matching Deny anywhere is final, whatever else matches.
            #[test]
            fn any_matching_deny_wins(mode in mode(), layers in layers(), action in action()) {
                let p = policy(mode, layers);
                let denied = p.layers.iter().flat_map(|l| &l.rules)
                    .any(|r| r.effect == Effect::Deny && r.matches(&action));
                if denied {
                    prop_assert_eq!(p.decide(&action).effect, Effect::Deny);
                }
            }

            /// D-2: within the deciding layer the most severe matching effect applies, and a
            /// lower layer never overrides a higher one that matched (except Deny above).
            #[test]
            fn topmost_matching_layer_decides_with_deny_over_ask_over_allow(
                layers in layers(), action in action()
            ) {
                prop_assume!(action.capability != Capability::Sensitive);
                let p = policy(Mode::FullAccess, layers);
                let any_deny = p.layers.iter().flat_map(|l| &l.rules)
                    .any(|r| r.effect == Effect::Deny && r.matches(&action));
                let top = p.layers.iter().rev().find_map(|l| {
                    l.rules.iter().filter(|r| r.matches(&action)).map(|r| r.effect).max()
                });
                let expected = if any_deny { Effect::Deny } else { top.unwrap_or(Effect::Allow) };
                prop_assert_eq!(p.decide(&action).effect, expected);
            }

            /// D-2: adding a session layer on top of agent defaults can change Ask/Allow but
            /// can never weaken an agent-level Deny.
            #[test]
            fn session_layer_never_weakens_agent_deny(
                mode in mode(), agent in prop::collection::vec(rule(), 0..5),
                session in prop::collection::vec(rule(), 0..5), action in action()
            ) {
                let base = policy(mode, vec![RuleLayer { rules: agent.clone() }]);
                let stacked = policy(mode, vec![RuleLayer { rules: agent }, RuleLayer { rules: session }]);
                if base.decide(&action).effect == Effect::Deny
                    && matches!(base.decide(&action).reason, Reason::DenyRule { .. }) {
                    prop_assert_eq!(stacked.decide(&action).effect, Effect::Deny);
                }
            }

            /// D-4: no combination of mode and rules auto-allows a sensitive action.
            #[test]
            fn sensitive_is_never_allowed(mode in mode(), layers in layers(), inside in any::<bool>()) {
                let p = policy(mode, layers);
                let action = Action::new(Capability::Sensitive, target(Capability::Sensitive, inside));
                prop_assert_ne!(p.decide(&action).effect, Effect::Allow);
            }

            /// D-1: ReadOnly never allows a mutating capability, whatever the rules say.
            #[test]
            fn read_only_never_allows_mutation(layers in layers(), action in action()) {
                let p = policy(Mode::ReadOnly, layers);
                if mutates(action.capability) {
                    prop_assert_eq!(p.decide(&action).effect, Effect::Deny);
                }
            }
        }
    }

    fn layer(rules: Vec<Rule>) -> RuleLayer {
        RuleLayer { rules }
    }

    #[test]
    fn guard_allows_inside_root_and_asks_outside() {
        let p = guard(&["/work/proj"], vec![]);
        assert_eq!(
            p.decide(&Action::new(Capability::FsWrite, "/work/proj/a.txt"))
                .effect,
            Effect::Allow
        );
        assert_eq!(
            p.decide(&Action::new(Capability::FsWrite, "/etc/passwd"))
                .effect,
            Effect::Ask
        );
        assert_eq!(
            p.decide(&Action::new(Capability::FsRead, "/work/project2/x"))
                .effect,
            Effect::Ask
        );
    }

    #[test]
    fn dotdot_and_relative_targets_never_count_as_inside() {
        let p = guard(&["/work/proj"], vec![]);
        assert_eq!(
            p.decide(&Action::new(Capability::FsRead, "/work/proj/../secret"))
                .effect,
            Effect::Ask
        );
        assert_eq!(
            p.decide(&Action::new(Capability::FsRead, "proj/a")).effect,
            Effect::Ask
        );
    }

    #[test]
    fn read_only_blocks_mutation_even_if_a_rule_allows_it() {
        let mut p = guard(
            &["/work"],
            vec![layer(vec![Rule::new(
                Effect::Allow,
                Capability::FsWrite,
                "/work/**",
            )])],
        );
        p.mode = Mode::ReadOnly;
        let d = p.decide(&Action::new(Capability::FsWrite, "/work/a"));
        assert_eq!(d.effect, Effect::Deny);
        assert_eq!(
            p.decide(&Action::new(Capability::FsRead, "/work/a")).effect,
            Effect::Allow
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Browse, "https://example.com"))
                .effect,
            Effect::Allow
        );
    }

    #[test]
    fn deny_beats_ask_and_allow_within_a_layer() {
        let p = guard(
            &[],
            vec![layer(vec![
                Rule::new(Effect::Allow, Capability::Browse, "https://*.example.com"),
                Rule::new(Effect::Ask, Capability::Browse, "https://*.example.com"),
                Rule::new(Effect::Deny, Capability::Browse, "https://bad.example.com"),
            ])],
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Browse, "https://bad.example.com"))
                .effect,
            Effect::Deny
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Browse, "https://ok.example.com"))
                .effect,
            Effect::Ask
        );
    }

    #[test]
    fn deny_in_a_lower_layer_cannot_be_overridden_by_session_allow() {
        let agent = layer(vec![Rule::new(
            Effect::Deny,
            Capability::Browse,
            "https://bank.example",
        )]);
        let session = layer(vec![Rule::new(
            Effect::Allow,
            Capability::Browse,
            "https://bank.example",
        )]);
        let p = guard(&[], vec![agent, session]);
        let d = p.decide(&Action::new(Capability::Browse, "https://bank.example"));
        assert_eq!(d.effect, Effect::Deny);
        assert_eq!(d.reason, Reason::DenyRule { layer: 0 });
    }

    #[test]
    fn session_layer_overrides_agent_ask_with_allow() {
        let agent = layer(vec![Rule::new(
            Effect::Ask,
            Capability::Browse,
            "https://*",
        )]);
        let session = layer(vec![Rule::new(
            Effect::Allow,
            Capability::Browse,
            "https://docs.example.com",
        )]);
        let p = guard(&[], vec![agent, session]);
        assert_eq!(
            p.decide(&Action::new(Capability::Browse, "https://docs.example.com"))
                .effect,
            Effect::Allow
        );
        assert_eq!(
            p.decide(&Action::new(
                Capability::Browse,
                "https://other.example.com"
            ))
            .effect,
            Effect::Ask
        );
    }

    #[test]
    fn sensitive_actions_are_never_auto_allowed() {
        for mode in [Mode::Guard, Mode::FullAccess] {
            let mut p = guard(
                &["/"],
                vec![layer(vec![Rule::new(
                    Effect::Allow,
                    Capability::Sensitive,
                    "**",
                )])],
            );
            p.mode = mode;
            let d = p.decide(&Action::new(
                Capability::Sensitive,
                "pay https://shop.example",
            ));
            assert_eq!(d.effect, Effect::Ask, "mode {mode:?}");
            assert_eq!(d.reason, Reason::SensitiveFloor);
        }
        let mut p = guard(&[], vec![]);
        p.mode = Mode::ReadOnly;
        assert_eq!(
            p.decide(&Action::new(Capability::Sensitive, "x")).effect,
            Effect::Deny
        );
    }

    #[test]
    fn sensitive_can_still_be_tightened_to_deny() {
        let p = guard(
            &[],
            vec![layer(vec![Rule::new(
                Effect::Deny,
                Capability::Sensitive,
                "**",
            )])],
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Sensitive, "post")).effect,
            Effect::Deny
        );
    }

    #[test]
    fn full_access_allows_everything_except_sensitive() {
        let mut p = guard(&[], vec![]);
        p.mode = Mode::FullAccess;
        assert_eq!(
            p.decide(&Action::new(Capability::FsWrite, "/anywhere"))
                .effect,
            Effect::Allow
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Exec, "/")).effect,
            Effect::Allow
        );
        assert_eq!(
            p.decide(&Action::new(Capability::VaultUse, "https://a.example"))
                .effect,
            Effect::Allow
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Sensitive, "msg")).effect,
            Effect::Ask
        );
    }

    #[test]
    fn default_mode_is_guard() {
        assert_eq!(Mode::default(), Mode::Guard);
    }

    #[test]
    fn effect_severity_order() {
        assert!(Effect::Allow < Effect::Ask && Effect::Ask < Effect::Deny);
    }

    #[test]
    fn rule_for_other_capability_does_not_match() {
        let p = guard(
            &[],
            vec![layer(vec![Rule::new(
                Effect::Deny,
                Capability::Fetch,
                "**",
            )])],
        );
        assert_eq!(
            p.decide(&Action::new(Capability::Browse, "https://a.example"))
                .effect,
            Effect::Allow
        );
    }
}
