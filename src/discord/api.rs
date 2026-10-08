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

/// The "🛑 Annuler" button row of an `/estimation25` waiting message.
fn cancel_button_components(custom_id: &str) -> serde_json::Value {
    serde_json::json!([
        {
            "type": 1, // ACTION_ROW
            "components": [
                {
                    "type": 2, // BUTTON
                    "style": 4, // DANGER (red)
                    "label": "Annuler",
                    "custom_id": custom_id,
                    "emoji": {
                        "name": "🛑"
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

/// Edits the original response with `content` and an "Annuler" button whose interaction
/// carries `custom_id`. Later content-only edits keep the button.
pub async fn send_followup_with_cancel(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
    custom_id: &str,
) -> Result<(), reqwest::Error> {
    let body = serde_json::json!({
        "content": content,
        "components": cancel_button_components(custom_id)
    });
    patch_followup(client, application_id, token, &body).await
}

/// Edits the original response with `content` and removes its buttons.
pub async fn send_followup_without_buttons(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
) -> Result<(), reqwest::Error> {
    let body = serde_json::json!({ "content": content, "components": [] });
    patch_followup(client, application_id, token, &body).await
}

/// Body of the component response replacing a cancelled waiting message (without its button).
#[must_use]
pub fn cancelled_message_body(content: &str) -> serde_json::Value {
    serde_json::json!({ "content": content, "components": [] })
}

#[cfg(test)]
mod tests {
    use super::{build_followup_body, cancel_button_components};

    #[test]
    fn cancel_button_carries_the_run() {
        let row = cancel_button_components("cancel_est:abc");
        assert_eq!(
            row[0]["components"][0]["custom_id"].as_str(),
            Some("cancel_est:abc")
        );
        assert_eq!(row[0]["components"][0]["style"].as_u64(), Some(4));
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
