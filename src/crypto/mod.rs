use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::Path;

pub struct Identity {
    inner: age::x25519::Identity,
}

impl Identity {
    pub fn generate() -> Self {
        Self {
            inner: age::x25519::Identity::generate(),
        }
    }

    pub fn to_string(&self) -> String {
        self.inner.to_string().expose_secret().to_string()
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let pem = std::fs::read_to_string(path)
            .with_context(|| format!("read identity: {}", path.display()))?;
        let identity = pem
            .parse::<age::x25519::Identity>()
            .map_err(|e| anyhow::anyhow!("parse identity: {e}"))?;
        Ok(Self { inner: identity })
    }

    pub fn recipient(&self) -> age::x25519::Recipient {
        self.inner.to_public()
    }
}

pub fn encrypt(plaintext: &[u8], identity: &Identity) -> Result<Vec<u8>> {
    let recipient = identity.recipient();
    let encryptor = age::Encryptor::with_recipients(vec![Box::new(recipient)])
        .expect("valid recipient");

    let mut ciphertext = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut ciphertext)
        .context("init encryptor")?;
    writer.write_all(plaintext).context("encrypt write")?;
    writer.finish().context("encrypt finish")?;
    Ok(ciphertext)
}

pub fn decrypt(ciphertext: &[u8], identity: &Identity) -> Result<Vec<u8>> {
    let decryptor = match age::Decryptor::new(ciphertext).context("init decryptor")? {
        age::Decryptor::Recipients(d) => d,
        _ => bail!("unexpected passphrase-based encryption"),
    };

    let mut plaintext = Vec::new();
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity.inner as &dyn age::Identity))
        .context("decrypt")?;
    reader.read_to_end(&mut plaintext).context("decrypt read")?;
    Ok(plaintext)
}
