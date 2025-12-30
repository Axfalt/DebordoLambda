use serde::{Deserialize, Serialize};
use crate::config::CommandOption;

/// Représente une interaction Discord entrante.
#[derive(Debug, Deserialize)]
pub struct DiscordInteraction {
    #[serde(rename = "type")]
    pub interaction_type: u8,
    
    #[allow(dead_code)]
    pub token: Option<String>,
    
    #[allow(dead_code)]
    pub application_id: Option<String>,
    
    pub data: Option<InteractionData>,
}

#[derive(Debug, Deserialize)]
pub struct InteractionData {
    #[allow(dead_code)]
    pub name: Option<String>,
    
    pub options: Option<Vec<CommandOption>>,
}

#[derive(Debug, Serialize)]
pub struct DiscordResponse {
    #[serde(rename = "type")]
    pub response_type: u8,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ResponseData>,
}

#[derive(Debug, Serialize)]
pub struct ResponseData {
    pub content: String,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
}

pub mod interaction_types {
    pub const PING: u8 = 1;
    
    pub const APPLICATION_COMMAND: u8 = 2;
}

pub mod response_types {
    pub const PONG: u8 = 1;
    
    pub const CHANNEL_MESSAGE_WITH_SOURCE: u8 = 4;
    
    #[allow(dead_code)]
    pub const DEFERRED_CHANNEL_MESSAGE_WITH_SOURCE: u8 = 5;
}

