fn view_config_button_body(content: &str, custom_id: &str) -> serde_json::Value {
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
                        "custom_id": custom_id,
                        "emoji": {
                            "name": "⚙️"
                        }
                    }
                ]
            }
        ]
    })
}

fn build_followup_body(content: &str) -> serde_json::Value {
    if content.contains("**Probabilité de mort:") {
        view_config_button_body(content, "vconf")
    } else if content.contains("Résultats de la simulation de réparation") {
        // Distinct custom_id (underscore, not "vconf:") so it never collides with
        // handle_component_interaction's `starts_with("vconf:")` debordo routing.
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

/// Envoie une réponse différée avec une pièce jointe texte (utilisé par /reparo pour la liste
/// des bâtiments) au lieu de l'inclure dans le contenu du message : pas affichée par défaut
/// (pièce jointe repliée, à ouvrir sur clic) et sans risque de dépasser la limite de longueur
/// d'un message Discord, quelle que soit la taille de la ville.
pub async fn send_followup_with_attachment(
    client: &reqwest::Client,
    application_id: &str,
    token: &str,
    content: &str,
    filename: &str,
    file_content: String,
) -> Result<(), reqwest::Error> {
    let mut body = build_followup_body(content);
    body["attachments"] = serde_json::json!([{ "id": 0, "filename": filename }]);

    let part = reqwest::multipart::Part::text(file_content)
        .file_name(filename.to_string())
        .mime_str("text/plain")
        .expect("text/plain is a valid mime type");

    let form = reqwest::multipart::Form::new()
        .text("payload_json", body.to_string())
        .part("files[0]", part);

    client
        .patch(followup_message_url(application_id, token))
        .multipart(form)
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
