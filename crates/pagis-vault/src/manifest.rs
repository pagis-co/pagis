//! The vault capability manifest (ADR-0005, ADR-0013).
//!
//! Five tools, all namespaced `vault`. The handle is the durable
//! credential ID, not a short-lived ticket: a confirm-password field
//! needs the same secret twice and a later run needs it again, so a
//! fill is replayable. The ID is not a secret; the Grant is the gate.
//!
//! No tool returns a secret, and there is no tool that reveals one.

use pagis_broker::{
    AllowRuleBuilder, ApprovalPresentation, CapabilityManifest, EffectClass, ManifestTool, ToolDef,
    ToolRoute, VaultTool,
};

pub const VAULT_NAMESPACE: &str = "vault";

pub const VAULT_LIST: &str = "vault__list";
pub const VAULT_CREATE: &str = "vault__create";
pub const VAULT_FILL: &str = "vault__fill";
pub const VAULT_TOTP_FILL: &str = "vault__totp_fill";
pub const VAULT_DELETE: &str = "vault__delete";

/// The vault tool one qualified name routes to.
pub fn tool_of(name: &str) -> Option<VaultTool> {
    match name {
        VAULT_LIST => Some(VaultTool::List),
        VAULT_CREATE => Some(VaultTool::Create),
        VAULT_FILL => Some(VaultTool::Fill),
        VAULT_TOTP_FILL => Some(VaultTool::TotpFill),
        VAULT_DELETE => Some(VaultTool::Delete),
        _ => None,
    }
}

fn tool(
    name: &str,
    description: &str,
    parameters: serde_json::Value,
    route: VaultTool,
    effect: EffectClass,
    presentation: Option<ApprovalPresentation>,
) -> ManifestTool {
    ManifestTool {
        definition: ToolDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
        capability: None,
        effect,
        presentation,
        route: ToolRoute::Vault { tool: route },
        call_timeout: None,
        visibility: Default::default(),
        widget: None,
    }
}

/// The approval presentation every credential-releasing tool renders. The card's
/// fields and its allow rule come from the Credential record, so
/// `body_argument` stays empty: nothing the model wrote reaches it.
fn credential_approval(title: &str) -> Option<ApprovalPresentation> {
    Some(ApprovalPresentation {
        action_title: title.to_string(),
        body_argument: None,
        allow_rule_builder: Some(AllowRuleBuilder::CredentialDomain),
    })
}

fn id_parameter() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "description": "The credential id from vault__list or vault__create."
    })
}

pub fn manifest() -> CapabilityManifest {
    CapabilityManifest {
        namespace: VAULT_NAMESPACE.to_string(),
        source_version: env!("CARGO_PKG_VERSION").to_string(),
        tools: vec![
            tool(
                VAULT_LIST,
                "List the saved logins in the workspace vault. Each entry carries an id, \
                 the site, and the username. No secret is ever returned.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "domain": {
                            "type": "string",
                            "description": "Only list logins for this registrable domain, e.g. example.com."
                        }
                    }
                }),
                VaultTool::List,
                EffectClass::Free,
                None,
            ),
            tool(
                VAULT_CREATE,
                "Save a new login for a site you are signing up to. Pagis mints a unique \
                 random password, stores it, and never shows it to you. `login_url` is the \
                 one address a later fill opens, and its registrable domain must equal \
                 `domain`.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "domain": {
                            "type": "string",
                            "description": "The site's registrable domain, e.g. example.com."
                        },
                        "username": {
                            "type": "string",
                            "description": "The username or email address the account uses."
                        },
                        "login_url": {
                            "type": "string",
                            "description": "The sign-in address on that domain, e.g. https://example.com/login."
                        },
                        "rules": {
                            "type": "string",
                            "description": "The site's published password rules, in Apple's Password Rules grammar, e.g. \"minlength: 8; maxlength: 16; required: lower, upper, digit\". Describe only what the site forbids; Pagis mints the strongest password those rules permit."
                        }
                    },
                    "required": ["domain", "username", "login_url"]
                }),
                VaultTool::Create,
                EffectClass::CredentialRelease,
                credential_approval("Create a login"),
            ),
            tool(
                VAULT_FILL,
                "Sign in with a saved login. Pagis opens the credential's own sign-in \
                 address in its own browser tab and checks that the page is https on the \
                 credential's site. It fills the username and the password into the \
                 login form of the page. \
                 It does not fill the page you are on. It reports that it filled the \
                 fields, or that it failed and why. Submit the form yourself.",
                serde_json::json!({
                    "type": "object",
                    "properties": {"id": id_parameter()},
                    "required": ["id"]
                }),
                VaultTool::Fill,
                EffectClass::CredentialRelease,
                credential_approval("Fill a saved login"),
            ),
            tool(
                VAULT_TOTP_FILL,
                "Fill the current one-time code for a saved login into the code field \
                 of the page in Pagis's own tab. Use it on the code step of a \
                 sign-in, after the password was accepted. The tab must still show the \
                 credential's own site over https. It reports that it filled the field, \
                 or that it failed and why.",
                serde_json::json!({
                    "type": "object",
                    "properties": {"id": id_parameter()},
                    "required": ["id"]
                }),
                VaultTool::TotpFill,
                EffectClass::CredentialRelease,
                credential_approval("Type a one-time code"),
            ),
            tool(
                VAULT_DELETE,
                "Delete a saved login. Use it when a site rejected the password Pagis \
                 minted: delete the record and create it again with the site's rules.",
                serde_json::json!({
                    "type": "object",
                    "properties": {"id": id_parameter()},
                    "required": ["id"]
                }),
                VaultTool::Delete,
                EffectClass::Destructive,
                Some(ApprovalPresentation {
                    action_title: "Delete a saved login".to_string(),
                    body_argument: Some("id".to_string()),
                    allow_rule_builder: None,
                }),
            ),
        ],
        event_kinds: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_namespaced_and_routed() {
        let manifest = manifest();

        assert_eq!(manifest.namespace, VAULT_NAMESPACE);
        assert_eq!(manifest.tools.len(), 5);
        for tool in &manifest.tools {
            assert!(
                tool.definition.name.starts_with("vault__"),
                "{}",
                tool.definition.name
            );
            assert!(tool_of(&tool.definition.name).is_some());
        }
    }

    #[test]
    fn the_credential_releasing_tools_ask_and_build_a_domain_rule() {
        for name in [VAULT_CREATE, VAULT_FILL, VAULT_TOTP_FILL] {
            let tool = manifest()
                .tools
                .into_iter()
                .find(|tool| tool.definition.name == name)
                .expect("tool in the manifest");
            assert_eq!(tool.effect, EffectClass::CredentialRelease);
            assert!(tool.effect.requires_approval());
            let presentation = tool.presentation.expect("an approval presentation");
            assert_eq!(
                presentation.allow_rule_builder,
                Some(AllowRuleBuilder::CredentialDomain)
            );
            // Nothing the model wrote reaches the card body.
            assert_eq!(presentation.body_argument, None);
        }
    }

    #[test]
    fn listing_is_free_and_deleting_asks() {
        let tools = manifest().tools;
        let effect = |name: &str| {
            tools
                .iter()
                .find(|tool| tool.definition.name == name)
                .expect("tool in the manifest")
                .effect
        };

        assert_eq!(effect(VAULT_LIST), EffectClass::Free);
        assert_eq!(effect(VAULT_DELETE), EffectClass::Destructive);
        assert!(effect(VAULT_DELETE).requires_approval());
    }

    #[test]
    fn no_tool_description_offers_to_reveal_a_secret() {
        for tool in manifest().tools {
            let text = tool.definition.description.to_ascii_lowercase();
            assert!(
                !text.contains("returns the password") && !text.contains("show the password"),
                "{}",
                tool.definition.name
            );
        }
    }
}
