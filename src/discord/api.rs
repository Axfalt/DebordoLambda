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

    let body = serde_json::json!({
        "content": content
    });

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
