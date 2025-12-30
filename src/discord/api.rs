#[allow(dead_code)]
pub async fn send_followup(
    application_id: &str,
    token: &str,
    content: &str,
) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://discord.com/api/v10/webhooks/{}/{}/messages/@original",
        application_id, token
    );

    let body = serde_json::json!({
        "content": content
    });

    client.patch(&url).json(&body).send().await?;

    Ok(())
}

#[allow(dead_code)]
pub async fn create_followup_message(
    application_id: &str,
    token: &str,
    content: &str,
) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://discord.com/api/v10/webhooks/{}/{}",
        application_id, token
    );

    let body = serde_json::json!({
        "content": content
    });

    client.post(&url).json(&body).send().await?;

    Ok(())
}

