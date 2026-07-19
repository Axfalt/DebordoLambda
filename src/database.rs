//! Helper module to encrypt and save user keys in DynamoDB using AWS KMS.

use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_kms::primitives::Blob;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use tracing::{info, error};

/// Encrypts a plaintext key using AWS KMS and stores it in the DynamoDB UserExternalIds table.
pub async fn store_user_key(
    user_id: &str,
    plaintext_key: &str,
    kms_client: &aws_sdk_kms::Client,
    db_client: &aws_sdk_dynamodb::Client,
) -> Result<(), lambda_runtime::Error> {
    info!("Encrypting API key for user {}", user_id);

    // 1. Get KMS Key ID from environment
    let kms_key_id = std::env::var("KMS_KEY_ID").unwrap_or_else(|_| "alias/aws/lambda".to_string());

    // 2. Encrypt plaintext key via KMS
    let encrypt_res = kms_client
        .encrypt()
        .key_id(kms_key_id)
        .plaintext(Blob::new(plaintext_key.as_bytes()))
        .send()
        .await
        .map_err(|e| {
            use aws_sdk_kms::error::ProvideErrorMetadata;
            let code = e.code().unwrap_or("unknown");
            let message = e.message().unwrap_or("no message");
            error!("KMS encryption failed: Code={}, Message={}, Debug={:?}", code, message, e);
            lambda_runtime::Error::from(format!("Encryption failed: {} ({})", message, code))
        })?;

    let ciphertext = encrypt_res
        .ciphertext_blob()
        .ok_or_else(|| lambda_runtime::Error::from("KMS did not return ciphertext blob"))?;

    // 3. Encode ciphertext as base64
    let encoded_key = STANDARD.encode(ciphertext.as_ref());

    // 4. Save to DynamoDB
    let table_name = std::env::var("USER_TABLE_NAME").unwrap_or_else(|_| "UserExternalIds".to_string());
    info!("Storing encrypted key in DynamoDB table {}", table_name);

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

    info!("Successfully stored encrypted API key for user {}", user_id);
    Ok(())
}
