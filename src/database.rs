//! Helper module to encrypt and save user keys in DynamoDB using local AES-256-GCM client-side,
//! with the key passphrase fetched dynamically at runtime from AWS SSM Parameter Store.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use aws_sdk_dynamodb::types::AttributeValue;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::RngExt;
use sha2::{Digest, Sha256};
use tracing::{error, info};

/// Helper function to retrieve the encryption passphrase from SSM and derive a 32-byte key.
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

/// Encrypts a plaintext key using local AES-256-GCM (with key fetched from AWS SSM Parameter Store)
/// and stores it in the DynamoDB UserExternalIds table.
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

/// Retrieves and decrypts the API key for a user from DynamoDB.
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
