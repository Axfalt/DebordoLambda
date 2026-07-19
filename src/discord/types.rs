// Shared between bootstrap and worker binaries — suppress dead-code lints.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use crate::config::CommandOption;

/// Représente une interaction Discord entrante.
#[derive(Debug, Deserialize)]
pub struct DiscordInteraction {
    #[serde(rename = "type")]
    pub interaction_type: u8,
    
    pub token: Option<String>,
    pub application_id: Option<String>,
    pub data: Option<InteractionData>,
    pub member: Option<Member>,
    pub user: Option<User>,
}

#[derive(Debug, Deserialize)]
pub struct Member {
    pub user: User,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct InteractionData {
    pub name: Option<String>,
    pub options: Option<Vec<CommandOption>>,
    pub custom_id: Option<String>,
    pub components: Option<Vec<ModalActionRow>>,
}

#[derive(Debug, Deserialize)]
pub struct ModalActionRow {
    #[serde(rename = "type")]
    pub component_type: u8,
    pub components: Vec<ModalComponent>,
}

#[derive(Debug, Deserialize)]
pub struct ModalComponent {
    #[serde(rename = "type")]
    pub component_type: u8,
    pub custom_id: String,
    pub value: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiscordResponse {
    #[serde(rename = "type")]
    pub response_type: u8,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl DiscordInteraction {
    /// Récupère l'ID de l'utilisateur Discord qui a déclenché l'interaction.
    pub fn user_id(&self) -> Option<&str> {
        if let Some(user) = &self.user {
            Some(&user.id)
        } else if let Some(member) = &self.member {
            Some(&member.user.id)
        } else {
            None
        }
    }

    /// Récupère la valeur saisie dans un champ de formulaire modal.
    pub fn get_modal_value(&self, custom_id: &str) -> Option<&str> {
        self.data.as_ref()
            .and_then(|d| d.components.as_ref())
            .and_then(|rows| {
                for row in rows {
                    for comp in &row.components {
                        if comp.custom_id == custom_id {
                            return comp.value.as_deref();
                        }
                    }
                }
                None
            })
    }
}

pub mod interaction_types {
    pub const PING: u8 = 1;
    pub const APPLICATION_COMMAND: u8 = 2;
    pub const MESSAGE_COMPONENT: u8 = 3;
    pub const APPLICATION_COMMAND_AUTOCOMPLETE: u8 = 4;
    pub const MODAL_SUBMIT: u8 = 5;
}

pub mod response_types {
    pub const PONG: u8 = 1;
    pub const CHANNEL_MESSAGE_WITH_SOURCE: u8 = 4;
    pub const DEFERRED_CHANNEL_MESSAGE_WITH_SOURCE: u8 = 5;
    pub const DEFERRED_UPDATE_MESSAGE: u8 = 6;
    pub const UPDATE_MESSAGE: u8 = 7;
    pub const APPLICATION_COMMAND_AUTOCOMPLETE_RESULT: u8 = 8;
    pub const MODAL: u8 = 9;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_user_id_from_user() {
        let interaction_json = json!({
            "type": 2,
            "user": {
                "id": "123456789"
            }
        });
        let interaction: DiscordInteraction = serde_json::from_value(interaction_json).unwrap();
        assert_eq!(interaction.user_id(), Some("123456789"));
    }

    #[test]
    fn test_parse_user_id_from_member() {
        let interaction_json = json!({
            "type": 2,
            "member": {
                "user": {
                    "id": "987654321"
                }
            }
        });
        let interaction: DiscordInteraction = serde_json::from_value(interaction_json).unwrap();
        assert_eq!(interaction.user_id(), Some("987654321"));
    }

    #[test]
    fn test_parse_modal_submit() {
        let interaction_json = json!({
            "type": 5,
            "data": {
                "custom_id": "register_key_modal",
                "components": [
                    {
                        "type": 1,
                        "components": [
                            {
                                "type": 4,
                                "custom_id": "api_key_input",
                                "value": "secret_key_123"
                            }
                        ]
                    }
                ]
            }
        });
        let interaction: DiscordInteraction = serde_json::from_value(interaction_json).unwrap();
        assert_eq!(interaction.interaction_type, interaction_types::MODAL_SUBMIT);
        assert_eq!(interaction.get_modal_value("api_key_input"), Some("secret_key_123"));
        assert_eq!(interaction.get_modal_value("non_existent"), None);
    }
}



