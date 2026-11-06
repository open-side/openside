use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Browser profile new CLI and MCP sessions on this computer use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CliProfile {
    /// The managed profile with the person's logins.
    Shared,
    /// Own cookies and storage, wiped when the session is archived or deleted.
    Isolated,
    /// A private browser whose browsing is never remembered.
    Incognito,
}

impl CliProfile {
    /// Every profile, in the order clients list them.
    pub const ALL: [Self; 3] = [Self::Shared, Self::Isolated, Self::Incognito];

    /// The id `cli.toml`, the CLI's `--profile` and this API use.
    pub fn id(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Isolated => "isolated",
            Self::Incognito => "incognito",
        }
    }

    /// The profile whose [`id`](Self::id) is `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.id() == id)
    }
}

/// `GET /v1/settings/cli`: the defaults `<config dir>/cli.toml` holds for new CLI and MCP
/// sessions on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CliDefaults {
    /// The saved default; `shared` (the account's first profile) when none is saved.
    pub profile: CliProfile,
    /// True when no profile is saved, so new sessions use the account's first profile.
    pub profile_is_default: bool,
}

/// `PATCH /v1/settings/cli`; changing it needs local access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateCliDefaultsRequest {
    /// New default; `null` clears it (back to the account's first profile). Required: an
    /// omitted field is refused rather than read as a clear.
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(schema_with = "nullable_profile", required)]
    pub profile: Option<CliProfile>,
}

/// `CliProfile` or null. A required `Option` field otherwise loses its null in the schema.
fn nullable_profile(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<Option<CliProfile>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cli_profile_ids_are_the_wire_names_and_the_update_needs_an_explicit_profile() {
        for profile in CliProfile::ALL {
            assert_eq!(serde_json::to_value(profile).unwrap(), json!(profile.id()));
            assert_eq!(CliProfile::from_id(profile.id()), Some(profile));
        }
        assert_eq!(CliProfile::from_id("private"), None);
        let update = |body| serde_json::from_value::<UpdateCliDefaultsRequest>(body);
        assert_eq!(
            update(json!({"profile": "isolated"})).unwrap().profile,
            Some(CliProfile::Isolated)
        );
        assert_eq!(update(json!({"profile": null})).unwrap().profile, None);
        assert!(update(json!({})).is_err());
        assert!(update(json!({"profile": "private"})).is_err());
        assert!(update(json!({"profile": null, "host": "work"})).is_err());

        let schema = serde_json::to_value(schemars::schema_for!(UpdateCliDefaultsRequest)).unwrap();
        assert_eq!(schema["required"], json!(["profile"]));
        assert!(schema.to_string().contains("\"null\""), "{schema}");
    }
}
