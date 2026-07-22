fn build_followup_body(content: &str) -> serde_json::Value {
    let has_simulation_results = content.contains("**Probabilité de mort:");

    if has_simulation_results {
        serde_json::json!({
            "content": content,
            "components": [
                {
                    "type": 1, // ACTION_ROW
                    "components": [
                        {
                            "type": 2, // BUTTON
                            "style": 2, // SECONDARY (grey)
                            "label": "Voir la configuration",
                            "custom_id": "vconf",
                            "emoji": {
                                "name": "⚙️"
                            }
                        }
                    ]
                }
            ]
        })
    } else {
        serde_json::json!({
            "content": content
        })
    }
}

pub async fn send_followup(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
) -> Result<(), reqwest::Error> {
    let url = format!(
        "https://discord.com/api/v10/webhooks/{}/{}/messages/@original",
        application_id, token
    );

    let body = build_followup_body(content);

    client
        .patch(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::build_followup_body;

    #[test]
    fn adds_config_button_for_simulation_results() {
        let body = build_followup_body("💀 **Probabilité de mort: 12.500%**");

        assert!(body.get("components").is_some());
    }

    #[test]
    fn omits_config_button_for_non_result_messages() {
        let body = build_followup_body("⏱️ La simulation a expiré.");

        assert!(body.get("components").is_none());
    }
}
