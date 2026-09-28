/// The "⚙️ Voir la configuration" button row shared by the result messages.
fn config_button_components(custom_id: &str) -> serde_json::Value {
    serde_json::json!([
        {
            "type": 1, // ACTION_ROW
            "components": [
                {
                    "type": 2, // BUTTON
                    "style": 2, // SECONDARY (grey)
                    "label": "Voir la configuration",
                    "custom_id": custom_id,
                    "emoji": {
                        "name": "⚙️"
                    }
                }
            ]
        }
    ])
}

fn view_config_button_body(content: &str, custom_id: &str) -> serde_json::Value {
    serde_json::json!({
        "content": content,
        "components": config_button_components(custom_id)
    })
}

fn build_followup_body(content: &str) -> serde_json::Value {
    if content.contains("**Probabilité de mort:") || content.contains("**Défense requise:") {
        view_config_button_body(content, "vconf")
    } else if content.contains("Résultats de la simulation de réparation") {
        view_config_button_body(content, "vconf_reparo")
    } else {
        serde_json::json!({
            "content": content
        })
    }
}

fn followup_message_url(application_id: &str, token: &str) -> String {
    format!(
        "https://discord.com/api/v10/webhooks/{}/{}/messages/@original",
        application_id, token
    )
}

async fn patch_followup(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    body: &serde_json::Value,
) -> Result<(), reqwest::Error> {
    client
        .patch(followup_message_url(application_id, token))
        .json(body)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

pub async fn send_followup(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
) -> Result<(), reqwest::Error> {
    let body = build_followup_body(content);
    patch_followup(client, application_id, token, &body).await
}

pub async fn create_followup_message(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
) -> Result<(), reqwest::Error> {
    let url = format!(
        "https://discord.com/api/v10/webhooks/{}/{}",
        application_id, token
    );

    let body = serde_json::json!({
        "content": content
    });

    client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

/// Body of a message that mentions (and notifies) only `user_id`, with an optional
/// "Voir la configuration" button.
fn mention_body(content: &str, user_id: &str, button: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "content": content,
        "allowed_mentions": { "parse": [], "users": [user_id] }
    });
    if let Some(custom_id) = button {
        body["components"] = config_button_components(custom_id);
    }
    body
}

/// Posts a new follow-up message that mentions `user_id`. Unlike an edit of the original
/// response, a new message notifies the mentioned user.
pub async fn post_followup_mentioning(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
    user_id: &str,
    button: Option<&str>,
) -> Result<(), reqwest::Error> {
    client
        .post(format!(
            "https://discord.com/api/v10/webhooks/{}/{}",
            application_id, token
        ))
        .json(&mention_body(content, user_id, button))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Edits the original response with `content` and a "Voir la configuration" button whose
/// interaction carries `custom_id`.
pub async fn send_followup_with_button(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
    custom_id: &str,
) -> Result<(), reqwest::Error> {
    let body = view_config_button_body(content, custom_id);
    patch_followup(client, application_id, token, &body).await
}

/// Deletes the original (deferred) response of the interaction.
pub async fn delete_original(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
) -> Result<(), reqwest::Error> {
    client
        .delete(followup_message_url(application_id, token))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_followup_body, mention_body};

    #[test]
    fn mention_body_carries_the_config_button() {
        let body = mention_body("<@42> prêt", "42", Some("vconf_est:abc"));
        assert_eq!(
            body["components"][0]["components"][0]["custom_id"].as_str(),
            Some("vconf_est:abc")
        );
        assert!(mention_body("x", "42", None).get("components").is_none());
    }

    #[test]
    fn mention_body_only_allows_the_caller() {
        let body = mention_body("<@42> prêt", "42", None);
        assert_eq!(body["allowed_mentions"]["users"][0].as_str(), Some("42"));
        assert!(
            body["allowed_mentions"]["parse"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn adds_config_button_for_simulation_results() {
        let body = build_followup_body("💀 **Probabilité de mort: 12.500%**");

        assert!(body.get("components").is_some());
    }

    #[test]
    fn adds_config_button_for_defense_search_results() {
        let body = build_followup_body(
            "🛡️ **Défense requise: 5230** (probabilité de mort estimée: 4.912%)",
        );

        assert_eq!(
            body["components"][0]["components"][0]["custom_id"].as_str(),
            Some("vconf")
        );
    }

    #[test]
    fn omits_config_button_for_non_result_messages() {
        let body = build_followup_body("⏱️ La simulation a expiré.");

        assert!(body.get("components").is_none());
    }

    #[test]
    fn adds_distinct_config_button_for_reparo_results() {
        let body = build_followup_body("## 🔧 Résultats de la simulation de réparation\n\n...");

        let custom_id = body["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap();
        assert_eq!(custom_id, "vconf_reparo");
        assert_ne!(custom_id, "vconf");
        assert!(!custom_id.starts_with("vconf:"));
    }
}
