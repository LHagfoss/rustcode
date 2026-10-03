use super::{AccountStatus, Result, config_dir};
use anyhow::{Context, anyhow};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

const KEYRING_SERVICE: &str = "org.rustcode.provider-auth";
const ACCOUNT_FILE: &str = "provider-accounts.json";

pub trait CredentialStore: Send + Sync {
    fn get_secret(&self, provider: &str, account: &str, kind: &str) -> Result<String>;
    fn set_secret(&self, provider: &str, account: &str, kind: &str, value: &str) -> Result<()>;
    fn delete_secret(&self, provider: &str, account: &str, kind: &str) -> Result<()>;
}

#[derive(Default)]
pub struct NativeCredentialStore;

impl NativeCredentialStore {
    pub(crate) fn global() -> &'static Self {
        static STORE: OnceLock<NativeCredentialStore> = OnceLock::new();
        STORE.get_or_init(Self::default)
    }

    pub async fn get(&self, provider: &str, account: &str, kind: &str) -> Result<String> {
        let (provider, account, kind) = (provider.to_owned(), account.to_owned(), kind.to_owned());
        tokio::task::spawn_blocking(move || Self::default().get_secret(&provider, &account, &kind))
            .await
            .context("credential-store worker stopped")?
    }

    pub async fn set(
        &self,
        provider: &str,
        account: &str,
        kind: &str,
        value: String,
    ) -> Result<()> {
        let (provider, account, kind) = (provider.to_owned(), account.to_owned(), kind.to_owned());
        tokio::task::spawn_blocking(move || {
            Self::default().set_secret(&provider, &account, &kind, &value)
        })
        .await
        .context("credential-store worker stopped")?
    }

    pub async fn delete(&self, provider: &str, account: &str, kind: &str) -> Result<()> {
        let (provider, account, kind) = (provider.to_owned(), account.to_owned(), kind.to_owned());
        tokio::task::spawn_blocking(move || {
            Self::default().delete_secret(&provider, &account, &kind)
        })
        .await
        .context("credential-store worker stopped")?
    }
}

impl CredentialStore for NativeCredentialStore {
    fn get_secret(&self, provider: &str, account: &str, kind: &str) -> Result<String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key(provider, account, kind))
            .map_err(|_| anyhow!("could not open the operating system credential store"))?;
        entry.get_password().map_err(|_| {
            anyhow!("credential is missing or the operating system credential store is unavailable")
        })
    }

    fn set_secret(&self, provider: &str, account: &str, kind: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key(provider, account, kind))
            .map_err(|_| anyhow!("could not open the operating system credential store"))?;
        entry.set_password(value).map_err(|_| anyhow!("could not store credential in the operating system credential store; check that its keychain or Secret Service is unlocked"))
    }

    fn delete_secret(&self, provider: &str, account: &str, kind: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key(provider, account, kind))
            .map_err(|_| anyhow!("could not open the operating system credential store"))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(anyhow!(
                "could not remove credential from the operating system credential store"
            )),
        }
    }
}

fn key(provider: &str, account: &str, kind: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for value in [provider, account, kind] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    format!("credential-{}", hex::encode(hash.finalize()))
}

fn account_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(ACCOUNT_FILE))
}

fn read_accounts() -> Result<Vec<AccountStatus>> {
    let path = account_path()?;
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("could not parse {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn write_accounts(rows: &[AccountStatus]) -> Result<()> {
    let path = account_path()?;
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("credential metadata path has no directory"))?;
    fs::create_dir_all(directory)
        .with_context(|| format!("could not create {}", directory.display()))?;
    let bytes = serde_json::to_vec_pretty(rows).context("could not encode credential metadata")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .context("could not create temporary credential metadata file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("could not protect credential metadata file")?;
    }
    file.write_all(&bytes)
        .context("could not write credential metadata")?;
    file.as_file()
        .sync_all()
        .context("could not sync credential metadata")?;
    let temp_path = file.into_temp_path();
    crate::atomic_file::replace_file(&temp_path, &path)
        .context("could not atomically replace credential metadata")?;
    Ok(())
}

pub(super) fn load_accounts() -> Result<Vec<AccountStatus>> {
    read_accounts()
}

pub(super) fn upsert_account(value: AccountStatus) -> Result<()> {
    let _lock = metadata_lock()?;
    let mut rows = read_accounts()?;
    rows.retain(|row| {
        row.provider != value.provider || row.account != value.account || row.method != value.method
    });
    rows.push(value);
    write_accounts(&rows)
}

fn metadata_lock() -> Result<FileLock> {
    use fs2::FileExt as _;
    static LOCAL: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    let local = LOCAL
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| anyhow!("credential metadata lock was poisoned"))?;
    let dir = config_dir()?;
    fs::create_dir_all(&dir).context("could not create RustCode config directory")?;
    let path = dir.join("provider-accounts.lock");
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .context("could not open credential metadata lock")?;
    file.lock_exclusive()
        .context("could not lock credential metadata")?;
    Ok(FileLock {
        file,
        _local: local,
    })
}

struct FileLock {
    file: std::fs::File,
    _local: std::sync::MutexGuard<'static, ()>,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
