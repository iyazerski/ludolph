use anyhow::{Context, Result, ensure};
use ring::{digest, signature};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Manifest {
    pub version: String,
    pub sha256: String,
}

pub fn verify(key: &[u8], body: &[u8], signature: &[u8], version: &str) -> Result<Manifest> {
    signature::UnparsedPublicKey::new(&signature::RSA_PKCS1_2048_8192_SHA256, key)
        .verify(body, signature)
        .map_err(|_| anyhow::anyhow!("the update isn't signed by Ludolph's release key"))?;
    let manifest: Manifest = serde_json::from_slice(body).context("reading the signed update")?;
    ensure!(
        manifest.version == version,
        "the signed update is for a different version"
    );
    ensure!(
        manifest.sha256.len() == digest::SHA256_OUTPUT_LEN * 2
            && manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "the signed update has an invalid checksum"
    );
    Ok(manifest)
}

pub fn checksum(mut file: impl std::io::Read) -> Result<String> {
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{checksum, verify};
    use ring::rand::SystemRandom;
    use ring::signature::{KeyPair, RSA_PKCS1_SHA256, RsaKeyPair};

    #[test]
    fn accepts_only_the_signed_version_and_installer() {
        let payload = b"installer payload";
        let hash = checksum(&payload[..]).unwrap();
        let body = format!(r#"{{"version":"1.2.3","sha256":"{hash}"}}"#);
        let key = RsaKeyPair::from_pkcs8(include_bytes!("testdata/key.der")).unwrap();
        let public = key.public_key().as_ref();
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            body.as_bytes(),
            &mut signature,
        )
        .unwrap();
        let manifest = verify(public, body.as_bytes(), &signature, "1.2.3").unwrap();
        assert_eq!(manifest.sha256, hash);
        assert_ne!(manifest.sha256, checksum(&b"changed payload"[..]).unwrap());
        assert!(verify(public, body.as_bytes(), &signature, "1.2.4").is_err());
        let tampered = body.replace("1.2.3", "1.2.4");
        assert!(verify(public, tampered.as_bytes(), &signature, "1.2.4").is_err());
        signature[0] ^= 1;
        assert!(verify(public, body.as_bytes(), &signature, "1.2.3").is_err());
    }
}
