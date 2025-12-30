use ed25519_dalek::{Signature, Verifier, VerifyingKey};

pub fn verify_discord_signature(
    public_key: &str,
    signature: &str,
    timestamp: &str,
    body: &str,
) -> bool {
    // Décoder la clé publique
    let public_key_bytes = match hex::decode(public_key) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    // Décoder la signature
    let signature_bytes = match hex::decode(signature) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    // Construire la clé de vérification
    let verifying_key = match VerifyingKey::try_from(public_key_bytes.as_slice()) {
        Ok(key) => key,
        Err(_) => return false,
    };

    // Construire la signature
    let signature = match Signature::try_from(signature_bytes.as_slice()) {
        Ok(sig) => sig,
        Err(_) => return false,
    };

    // Vérifier: le message signé est timestamp + body
    let message = format!("{}{}", timestamp, body);
    verifying_key.verify(message.as_bytes(), &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_public_key() {
        assert!(!verify_discord_signature("invalid", "abc", "123", "body"));
    }

    #[test]
    fn test_invalid_signature_format() {
        // Clé publique valide format mais signature invalide
        let fake_key = "0".repeat(64);
        assert!(!verify_discord_signature(&fake_key, "invalid", "123", "body"));
    }
}

