//! Helper module to encrypt and save user keys in DynamoDB using local AES-256-GCM client-side,
//! with the key passphrase fetched dynamically at runtime from AWS SSM Parameter Store.

use aws_sdk_dynamodb::types::AttributeValue;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce
};
use sha2::{Sha256, Digest};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::RngExt;
use tracing::{info, error};

/// Encrypts a plaintext key using local AES-256-GCM (with key fetched from AWS SSM Parameter Store)
/// and stores it in the DynamoDB UserExternalIds table.
pub async fn store_user_key(
    user_id: &str,
    plaintext_key: &str,
    db_client: &aws_sdk_dynamodb::Client,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<(), lambda_runtime::Error> {
    info!("Encrypting API key client-side for user {}", user_id);

    // 1. Get SSM Parameter Name from environment
    let param_name = std::env::var("SSM_PARAMETER_NAME").unwrap_or_else(|_| "MH_EID_ENCRYPTION_KEY".to_string());
    info!("Fetching encryption passphrase from SSM parameter: {}", param_name);

    // 2. Fetch the passphrase from SSM Parameter Store (with decryption)
    let ssm_res = ssm_client
        .get_parameter()
        .name(param_name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|e| {
            error!("Failed to fetch parameter from SSM: {}", e);
            lambda_runtime::Error::from(format!("Failed to retrieve encryption key from SSM: {}", e))
        })?;

    let passphrase = ssm_res
        .parameter
        .and_then(|p| p.value)
        .ok_or_else(|| lambda_runtime::Error::from("SSM response did not contain parameter value"))?;

    if passphrase.trim().is_empty() {
        return Err(lambda_runtime::Error::from("Server configuration error: SSM passphrase is empty"));
    }

    // 3. Derive a 32-byte key from the passphrase using SHA-256
    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    let key_hash = hasher.finalize(); // 32 bytes

    // 4. Initialize AES-256-GCM cipher
    let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&key_hash);
    let cipher = Aes256Gcm::new(key);

    // 5. Generate a random 12-byte nonce
    let mut nonce_bytes = [0u8; 12];
    for byte in &mut nonce_bytes {
        *byte = rand::rng().random();
    }
    let nonce = Nonce::from_slice(&nonce_bytes);

    // 6. Encrypt the plaintext key
    let ciphertext = cipher
        .encrypt(nonce, plaintext_key.as_bytes())
        .map_err(|e| {
            error!("AES encryption failed: {:?}", e);
            lambda_runtime::Error::from(format!("Encryption failed: {:?}", e))
        })?;

    // 7. Prepend nonce to ciphertext to store them together
    let mut combined = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);

    // 8. Base64 encode the combined payload
    let encoded_key = STANDARD.encode(&combined);

    // 9. Save to DynamoDB
    let table_name = std::env::var("USER_TABLE_NAME").unwrap_or_else(|_| "UserExternalIds".to_string());
    info!("Storing encrypted key client-side in DynamoDB table {}", table_name);

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

    info!("Successfully stored encrypted API key client-side for user {}", user_id);
    Ok(())
}
