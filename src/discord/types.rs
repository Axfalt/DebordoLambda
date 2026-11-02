use crate::config::CommandOption;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct DiscordInteraction {
    #[serde(rename = "type")]
    pub interaction_type: u8,

    pub token: Option<String>,
    pub application_id: Option<String>,
    pub data: Option<InteractionData>,
    pub member: Option<Member>,
    pub user: Option<User>,
    pub message: Option<DiscordMessage>,
}

#[derive(Debug, Deserialize)]
pub struct DiscordMessage {
    pub content: Option<String>,
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
    pub resolved: Option<ResolvedData>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ResolvedData {
    #[serde(default)]
    pub attachments: std::collections::HashMap<String, Attachment>,
}

#[derive(Debug, Deserialize)]
pub struct Attachment {
    pub url: String,
    pub content_type: Option<String>,
    pub size: u64,
}

/// A top-level component of a submitted modal: an action row (`components`) or a label
/// (`component`).
#[derive(Debug, Deserialize)]
pub struct ModalActionRow {
    #[serde(rename = "type")]
    pub component_type: u8,
    #[serde(default)]
    pub components: Vec<ModalComponent>,
    #[serde(default)]
    pub component: Option<ModalComponent>,
}

#[derive(Debug, Deserialize)]
pub struct ModalComponent {
    #[serde(rename = "type")]
    pub component_type: u8,
    pub custom_id: String,
    pub value: Option<String>,
    /// Ids of the uploaded files of a file upload, found in the resolved attachments.
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DiscordResponse {
    #[serde(rename = "type")]
    pub response_type: u8,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl DiscordInteraction {
    pub fn user_id(&self) -> Option<&str> {
        self.user
            .as_ref()
            .map(|u| u.id.as_str())
            .or_else(|| self.member.as_ref().map(|m| m.user.id.as_str()))
    }

    /// The uploaded file an id from `get_modal_files` refers to.
    pub fn attachment(&self, id: &str) -> Option<&Attachment> {
        self.data.as_ref()?.resolved.as_ref()?.attachments.get(id)
    }

    fn get_modal_component(&self, custom_id: &str) -> Option<&ModalComponent> {
        self.data
            .as_ref()?
            .components
            .as_ref()?
            .iter()
            .flat_map(|row| row.components.iter().chain(&row.component))
            .find(|comp| comp.custom_id == custom_id)
    }

    pub fn get_modal_value(&self, custom_id: &str) -> Option<&str> {
        self.get_modal_component(custom_id)?.value.as_deref()
    }

    /// Ids of the files uploaded in a modal's file upload.
    pub fn get_modal_files(&self, custom_id: &str) -> &[String] {
        self.get_modal_component(custom_id)
            .map_or(&[], |comp| comp.values.as_slice())
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
    fn test_parse_modal_submit_with_labels_and_file() {
        let interaction_json = json!({
            "type": 5,
            "data": {
                "custom_id": "dm:api:comp",
                "components": [
                    {
                        "type": 18,
                        "id": 1,
                        "component": {
                            "type": 4,
                            "id": 2,
                            "custom_id": "defenses_input",
                            "value": "defense: 100"
                        }
                    },
                    {
                        "type": 18,
                        "id": 3,
                        "component": {
                            "type": 19,
                            "id": 4,
                            "custom_id": "capture_input",
                            "values": ["111"]
                        }
                    }
                ],
                "resolved": {
                    "attachments": {
                        "111": {
                            "id": "111",
                            "filename": "citoyens.png",
                            "url": "https://cdn.discordapp.com/ephemeral-attachments/1/111/citoyens.png",
                            "proxy_url": "https://media.discordapp.net/ephemeral-attachments/1/111/citoyens.png",
                            "content_type": "image/png",
                            "size": 12345,
                            "width": 600,
                            "height": 1292
                        }
                    }
                }
            }
        });
        let interaction: DiscordInteraction = serde_json::from_value(interaction_json).unwrap();
        assert_eq!(
            interaction.get_modal_value("defenses_input"),
            Some("defense: 100")
        );
        assert_eq!(interaction.get_modal_files("capture_input"), ["111"]);
        assert!(interaction.get_modal_files("defenses_input").is_empty());
        assert!(interaction.get_modal_files("non_existent").is_empty());
        let attachment = interaction.attachment("111").unwrap();
        assert_eq!(attachment.content_type.as_deref(), Some("image/png"));
        assert_eq!(attachment.size, 12345);
        assert!(interaction.attachment("222").is_none());
    }

    #[test]
    fn test_parse_modal_submit_without_a_file() {
        let interaction_json = json!({
            "type": 5,
            "data": {
                "custom_id": "dm:api:comp",
                "components": [
                    {"type": 18, "component": {"type": 4, "custom_id": "defenses_input", "value": "x"}},
                    {"type": 18, "component": {"type": 19, "custom_id": "capture_input", "values": []}}
                ]
            }
        });
        let interaction: DiscordInteraction = serde_json::from_value(interaction_json).unwrap();
        assert!(interaction.get_modal_files("capture_input").is_empty());
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
        assert_eq!(
            interaction.interaction_type,
            interaction_types::MODAL_SUBMIT
        );
        assert_eq!(
            interaction.get_modal_value("api_key_input"),
            Some("secret_key_123")
        );
        assert_eq!(interaction.get_modal_value("non_existent"), None);
    }
}
