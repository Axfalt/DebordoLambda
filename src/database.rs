use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use aws_sdk_dynamodb::types::AttributeValue;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::RngExt;
use sha2::{Digest, Sha256};
use tracing::{error, info};

async fn derive_key(ssm_client: &aws_sdk_ssm::Client) -> Result<[u8; 32], lambda_runtime::Error> {
    let param_name =
        std::env::var("SSM_PARAMETER_NAME").unwrap_or_else(|_| "MH_EID_ENCRYPTION_KEY".to_string());
    info!(
        "Fetching encryption passphrase from SSM parameter: {}",
        param_name
    );

    let ssm_res = ssm_client
        .get_parameter()
        .name(param_name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|e| {
            error!("Failed to fetch parameter from SSM: {}", e);
            lambda_runtime::Error::from(format!(
                "Failed to retrieve encryption key from SSM: {}",
                e
            ))
        })?;

    let passphrase = ssm_res.parameter.and_then(|p| p.value).ok_or_else(|| {
        lambda_runtime::Error::from("SSM response did not contain parameter value")
    })?;

    if passphrase.trim().is_empty() {
        return Err(lambda_runtime::Error::from(
            "Server configuration error: SSM passphrase is empty",
        ));
    }

    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    let mut key_hash = [0u8; 32];
    key_hash.copy_from_slice(&hasher.finalize());
    Ok(key_hash)
}

pub async fn store_user_key(
    user_id: &str,
    plaintext_key: &str,
    db_client: &aws_sdk_dynamodb::Client,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<(), lambda_runtime::Error> {
    info!("Encrypting API key client-side for user {}", user_id);

    // 1. Derive key from SSM
    let key_hash = derive_key(ssm_client).await?;

    // 2. Initialize AES-256-GCM cipher
    let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&key_hash);
    let cipher = Aes256Gcm::new(key);

    // 3. Generate a random 12-byte nonce
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // 4. Encrypt the plaintext key
    let ciphertext = cipher
        .encrypt(nonce, plaintext_key.as_bytes())
        .map_err(|e| {
            error!("AES encryption failed: {:?}", e);
            lambda_runtime::Error::from(format!("Encryption failed: {:?}", e))
        })?;

    // 5. Prepend nonce to ciphertext to store them together
    let mut combined = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);

    // 6. Base64 encode the combined payload
    let encoded_key = STANDARD.encode(&combined);

    // 7. Save to DynamoDB
    let table_name =
        std::env::var("USER_TABLE_NAME").unwrap_or_else(|_| "UserExternalIds".to_string());
    info!(
        "Storing encrypted key client-side in DynamoDB table {}",
        table_name
    );

    db_client
        .put_item()
        .table_name(table_name)
        .item("discord_user_id", AttributeValue::S(user_id.to_string()))
        .item("encrypted_key", AttributeValue::S(encoded_key))
        .send()
        .await
        .map_err(|e| {
            error!("DynamoDB put_item failed: {}", e);
            lambda_runtime::Error::from(format!("Database write failed: {}", e))
        })?;

    info!(
        "Successfully stored encrypted API key client-side for user {}",
        user_id
    );
    Ok(())
}

pub async fn get_user_key(
    user_id: &str,
    db_client: &aws_sdk_dynamodb::Client,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<Option<String>, lambda_runtime::Error> {
    info!("Retrieving API key for user {}", user_id);

    // 1. Fetch from DynamoDB
    let table_name =
        std::env::var("USER_TABLE_NAME").unwrap_or_else(|_| "UserExternalIds".to_string());
    let get_res = db_client
        .get_item()
        .table_name(table_name)
        .key("discord_user_id", AttributeValue::S(user_id.to_string()))
        .send()
        .await
        .map_err(|e| {
            error!("DynamoDB get_item failed: {}", e);
            lambda_runtime::Error::from(format!("Database read failed: {}", e))
        })?;

    let item = match get_res.item {
        Some(i) => i,
        None => {
            info!("No API key found in database for user {}", user_id);
            return Ok(None);
        }
    };

    let encoded_key = match item.get("encrypted_key").and_then(|v| v.as_s().ok()) {
        Some(k) => k,
        None => {
            error!(
                "DynamoDB record for user {} is missing 'encrypted_key' attribute",
                user_id
            );
            return Err(lambda_runtime::Error::from("Database record is corrupt"));
        }
    };

    // 2. Decode base64
    let combined = STANDARD.decode(encoded_key).map_err(|e| {
        error!("Base64 decoding failed for user {}: {}", user_id, e);
        lambda_runtime::Error::from("Decryption failed: invalid base64".to_string())
    })?;

    if combined.len() < 12 {
        error!("Decoded ciphertext is too short for user {}", user_id);
        return Err(lambda_runtime::Error::from("Database record is corrupt"));
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);

    // 3. Derive key from SSM
    let key_hash = derive_key(ssm_client).await?;

    // 4. Decrypt via AES-GCM
    let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&key_hash);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    let decrypted = cipher.decrypt(nonce, ciphertext).map_err(|e| {
        error!("AES decryption failed for user {}: {:?}", user_id, e);
        lambda_runtime::Error::from(format!("Decryption failed: {:?}", e))
    })?;

    let plaintext = String::from_utf8(decrypted).map_err(|e| {
        error!(
            "Decrypted key is not valid UTF-8 for user {}: {}",
            user_id, e
        );
        lambda_runtime::Error::from("Decrypted data is corrupt".to_string())
    })?;

    Ok(Some(plaintext))
}

// ---------------------------------------------------------------------------------------------
// `/estimation25` runs: the 2^32 seeds are split across parallel worker invocations, which merge
// their results in one DynamoDB item (table `ESTIMATION_TABLE_NAME`, key `run_id`, TTL
// `expires_at`). Every write is idempotent so SQS redeliveries cannot corrupt a run.
// ---------------------------------------------------------------------------------------------

const ESTIMATION_RUN_TTL_SECS: u64 = 24 * 3600;

fn estimation_table() -> String {
    std::env::var("ESTIMATION_TABLE_NAME").unwrap_or_else(|_| "EstimationRuns".to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Run id of an interaction: the interaction token is unique but secret, so only its hash is
/// stored.
pub fn estimation_run_id(token: &str) -> String {
    hex::encode(&Sha256::digest(token.as_bytes())[..16])
}

/// Creates the run item, keeping `config` (the readings as pasteable text) for the result's
/// "Voir la configuration" button. Returns `false` when the run already exists (a redelivered
/// plan job, which must not reset the run nor post over its result).
pub async fn create_estimation_run(
    run_id: &str,
    parts: u32,
    config: &str,
    db_client: &aws_sdk_dynamodb::Client,
) -> Result<bool, lambda_runtime::Error> {
    let now = now_secs();
    let created = db_client
        .put_item()
        .table_name(estimation_table())
        .item("run_id", AttributeValue::S(run_id.to_string()))
        .item("parts", AttributeValue::N(parts.to_string()))
        .item("started_at", AttributeValue::N(now.to_string()))
        .item(
            "expires_at",
            AttributeValue::N((now + ESTIMATION_RUN_TTL_SECS).to_string()),
        )
        .item("matches", AttributeValue::M(Default::default()))
        .item("config", AttributeValue::S(config.to_string()))
        .condition_expression("attribute_not_exists(run_id)")
        .send()
        .await;
    match created {
        Ok(_) => Ok(true),
        Err(e)
            if e.as_service_error()
                .is_some_and(|s| s.is_conditional_check_failed_exception()) =>
        {
            Ok(false)
        }
        Err(e) => {
            error!("DynamoDB put_item (estimation run) failed: {}", e);
            Err(lambda_runtime::Error::from(format!(
                "Database write failed: {}",
                e
            )))
        }
    }
}

/// Configuration (pasteable readings) of a run, `None` once the run has expired (TTL).
pub async fn get_estimation_config(
    run_id: &str,
    db_client: &aws_sdk_dynamodb::Client,
) -> Result<Option<String>, lambda_runtime::Error> {
    let item = db_client
        .get_item()
        .table_name(estimation_table())
        .key("run_id", AttributeValue::S(run_id.to_string()))
        .projection_expression("#config")
        .expression_attribute_names("#config", "config")
        .send()
        .await
        .map_err(|e| {
            error!("DynamoDB get_item (estimation config) failed: {}", e);
            lambda_runtime::Error::from(format!("Database read failed: {}", e))
        })?
        .item;
    Ok(item
        .as_ref()
        .and_then(|i| i.get("config"))
        .and_then(|v| v.as_s().ok())
        .cloned())
}

/// Whether part `index` may still update the waiting message: the run has not posted its
/// result and this part was not recorded before (a redelivered copy of a finished part must not
/// overwrite the result posted since).
pub async fn may_show_progress(
    run_id: &str,
    index: u32,
    db_client: &aws_sdk_dynamodb::Client,
) -> Result<bool, lambda_runtime::Error> {
    let checked = db_client
        .update_item()
        .table_name(estimation_table())
        .key("run_id", AttributeValue::S(run_id.to_string()))
        .update_expression("SET last_progress = :part")
        .condition_expression(
            "attribute_exists(run_id) AND attribute_not_exists(posted) \
             AND NOT contains(done, :part)",
        )
        .expression_attribute_values(":part", AttributeValue::N(index.to_string()))
        .send()
        .await;
    match checked {
        Ok(_) => Ok(true),
        Err(e)
            if e.as_service_error()
                .is_some_and(|s| s.is_conditional_check_failed_exception()) =>
        {
            Ok(false)
        }
        Err(e) => {
            error!("DynamoDB update_item (estimation progress) failed: {}", e);
            Err(lambda_runtime::Error::from(format!(
                "Database write failed: {}",
                e
            )))
        }
    }
}

/// All parts of a run are done: the merged matches, handed to exactly one caller.
#[derive(Debug)]
pub struct CompletedRun {
    pub matches: Vec<estimation25_lib::SeedMatch>,
    pub started_at: u64,
}

/// Records the matches of part `index`; returns the whole run once every part is recorded, to
/// the single caller that wins the right to post the result.
pub async fn record_estimation_part(
    run_id: &str,
    index: u32,
    matches: &[estimation25_lib::SeedMatch],
    db_client: &aws_sdk_dynamodb::Client,
) -> Result<Option<CompletedRun>, lambda_runtime::Error> {
    let table = estimation_table();
    let key = AttributeValue::S(run_id.to_string());
    let json = serde_json::to_string(matches)?;

    // `ADD` to a number set and `SET` of this part's slot are both idempotent.
    let updated = db_client
        .update_item()
        .table_name(&table)
        .key("run_id", key.clone())
        .update_expression("ADD done :part SET matches.#part = :matches")
        .expression_attribute_names("#part", format!("p{index}"))
        .expression_attribute_values(":part", AttributeValue::Ns(vec![index.to_string()]))
        .expression_attribute_values(":matches", AttributeValue::S(json))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
        .send()
        .await
        .map_err(|e| {
            error!("DynamoDB update_item (estimation part) failed: {}", e);
            lambda_runtime::Error::from(format!("Database write failed: {}", e))
        })?;
    let attrs = updated.attributes.unwrap_or_default();

    let parts = attrs
        .get("parts")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let done = attrs
        .get("done")
        .and_then(|v| v.as_ns().ok())
        .map_or(0, Vec::len);
    if done < parts {
        return Ok(None);
    }

    // Several callers can see the run complete (simultaneous finishes, redeliveries): only the
    // one setting `posted` first reports it.
    let claim = db_client
        .update_item()
        .table_name(&table)
        .key("run_id", key)
        .update_expression("SET posted = :true")
        .condition_expression("attribute_not_exists(posted)")
        .expression_attribute_values(":true", AttributeValue::Bool(true))
        .send()
        .await;
    if let Err(e) = claim {
        if e.as_service_error()
            .is_some_and(|s| s.is_conditional_check_failed_exception())
        {
            return Ok(None);
        }
        error!("DynamoDB update_item (estimation claim) failed: {}", e);
        return Err(lambda_runtime::Error::from(format!(
            "Database write failed: {}",
            e
        )));
    }

    let mut all = Vec::new();
    if let Some(slots) = attrs.get("matches").and_then(|v| v.as_m().ok()) {
        for slot in slots.values() {
            if let Ok(json) = slot.as_s() {
                all.extend(serde_json::from_str::<Vec<estimation25_lib::SeedMatch>>(
                    json,
                )?);
            }
        }
    }
    let started_at = attrs
        .get("started_at")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(now_secs);
    Ok(Some(CompletedRun {
        matches: all,
        started_at,
    }))
}

/// Claims the right to post for a run that has not posted yet (the watchdog). Returns the number
/// of finished parts, or `None` when the result (or another report) was already posted.
pub async fn claim_estimation_run(
    run_id: &str,
    db_client: &aws_sdk_dynamodb::Client,
) -> Result<Option<usize>, lambda_runtime::Error> {
    let claimed = db_client
        .update_item()
        .table_name(estimation_table())
        .key("run_id", AttributeValue::S(run_id.to_string()))
        .update_expression("SET posted = :true")
        .condition_expression("attribute_exists(run_id) AND attribute_not_exists(posted)")
        .expression_attribute_values(":true", AttributeValue::Bool(true))
        .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
        .send()
        .await;
    match claimed {
        Ok(out) => Ok(Some(
            out.attributes
                .as_ref()
                .and_then(|a| a.get("done"))
                .and_then(|v| v.as_ns().ok())
                .map_or(0, Vec::len),
        )),
        Err(e)
            if e.as_service_error()
                .is_some_and(|s| s.is_conditional_check_failed_exception()) =>
        {
            Ok(None)
        }
        Err(e) => {
            error!("DynamoDB update_item (estimation watchdog) failed: {}", e);
            Err(lambda_runtime::Error::from(format!(
                "Database write failed: {}",
                e
            )))
        }
    }
}

/// Seconds elapsed since `started_at` (for the result footer).
pub fn seconds_since(started_at: u64) -> u64 {
    now_secs().saturating_sub(started_at)
}
