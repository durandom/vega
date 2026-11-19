//! Chromium 146 ServerCertificate v1. Internal format: reject unknown versions,
//! schemas and metadata rather than migrating or replacing user trust decisions.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};

pub(super) const DEFAULT_SHA256: &str =
    "4678db0942d0b9e732122507bd6cdc08d128ab832da274ad7d46f8c3576bc925";
// CertificateMetadata.trust (field 1), CertificateTrust.trust_type (field 1) = 3.
const TRUSTED: &[u8] = &[0x0a, 0x02, 0x08, 0x03];

fn db_error(e: rusqlite::Error) -> Failure {
    fail(format!("VPN certificate database: {e}; stop the profile and check Chromium's ServerCertificate schema"))
}

fn hash(der: &[u8]) -> String {
    format!("{:x}", Sha256::digest(der))
}

fn schema(db: &Connection) -> Result<(), Failure> {
    for key in ["version", "last_compatible_version"] {
        let version: String = db
            .query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
            .map_err(db_error)?;
        if version != "1" {
            return Err(fail(
                "unsupported VPN ServerCertificate version; refusing to modify it",
            ));
        }
    }
    let mut stmt = db
        .prepare("PRAGMA table_info(certificates)")
        .map_err(db_error)?;
    let cols = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let expected = vec![
        ("sha256hash_hex".into(), "TEXT".into(), 0, 1),
        ("der_cert".into(), "BLOB".into(), 1, 0),
        ("trust_settings".into(), "BLOB".into(), 1, 0),
    ];
    if cols != expected {
        return Err(fail(
            "unsupported VPN ServerCertificate schema; refusing to modify it",
        ));
    }
    Ok(())
}

fn present(db: &Connection, pin: &str) -> Result<bool, Failure> {
    let row: Option<(Vec<u8>, Vec<u8>)> = db
        .query_row(
            "SELECT der_cert,trust_settings FROM certificates WHERE sha256hash_hex=?",
            [pin],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(false),
        Some((der, metadata)) if hash(&der) == pin && metadata == TRUSTED => Ok(true),
        Some(_) => Err(fail("pinned CA has conflicting or unsupported trust metadata; review it in the VPN certificate manager (no overwrite)")),
    }
}

impl Vpn {
    fn cert_path(&self) -> Result<PathBuf, Failure> {
        let path = self.profile.join("Default/ServerCertificate");
        // resolved() canonicalizes; explicitly reject aliases inside the profile.
        for p in path
            .ancestors()
            .take_while(|p| p.starts_with(&self.profile))
        {
            if fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(fail("VPN certificate path must not contain symlinks"));
            }
        }
        Ok(path)
    }

    pub(super) fn ca_trusted(&self) -> Result<bool, Failure> {
        let path = self.cert_path()?;
        if !path.exists() {
            return Ok(false);
        }
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(db_error)?;
        schema(&db)?;
        present(&db, &self.ca_sha256)
    }

    pub(super) fn trust(&self) -> Result<(), Failure> {
        if self.ca_trusted()? {
            return Ok(());
        }
        self.require_stopped_for_trust()?;
        let output = Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", &self.host,
                "security find-certificate -a -p -c 'Internal Root CA' /Library/Keychains/System.keychain"])
            .stdin(Stdio::null()).output().map_err(|e| fail(format!("fetch VPN CA: {e}")))?;
        if !output.status.success() {
            return Err(fail(
                "cannot read public VPN CA; check BatchMode SSH access and puck's System.keychain",
            ));
        }
        let pem = String::from_utf8(output.stdout).map_err(|_| fail("invalid CA PEM response"))?;
        let mut selected = None;
        for part in pem.split("-----BEGIN CERTIFICATE-----").skip(1) {
            let body = part
                .split_once("-----END CERTIFICATE-----")
                .ok_or_else(|| fail("invalid CA PEM response"))?
                .0;
            let der = STANDARD
                .decode(body.split_whitespace().collect::<String>())
                .map_err(|_| fail("invalid CA PEM response"))?;
            if hash(&der) == self.ca_sha256 {
                selected = Some(der);
            }
        }
        let der = selected
            .ok_or_else(|| fail("VPN CA SHA-256 fingerprint mismatch; no certificate imported"))?;
        self.require_stopped_for_trust()?;
        let path = self.cert_path()?;
        fs::create_dir_all(path.parent().expect("Default directory"))
            .map_err(|e| fail(e.to_string()))?;
        let exists = path.exists();
        let mut db = Connection::open(&path).map_err(db_error)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if !exists {
            tx.execute_batch("CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY,value LONGVARCHAR);
                INSERT INTO meta VALUES('version','1'),('last_compatible_version','1');
                CREATE TABLE certificates(sha256hash_hex TEXT PRIMARY KEY,der_cert BLOB NOT NULL,trust_settings BLOB NOT NULL);")
                .map_err(db_error)?;
        }
        schema(&tx)?;
        if !present(&tx, &self.ca_sha256)? {
            tx.execute(
                "INSERT INTO certificates VALUES(?,?,?)",
                params![self.ca_sha256, der, TRUSTED],
            )
            .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }

    fn require_stopped_for_trust(&self) -> Result<(), Failure> {
        let output = Command::new("ps")
            .args(["-ww", "-eo", "pid=,etime=,command="])
            .output()
            .map_err(|_| fail("cannot verify VPN profile is stopped"))?;
        if !output.status.success() {
            return Err(fail("cannot verify VPN profile is stopped"));
        }
        let running = procs::parse(&String::from_utf8_lossy(&output.stdout))
            .iter()
            .any(|p| {
                let argv: Vec<_> = p.cmd.split_whitespace().collect();
                let matches = |candidate: &str| {
                    resolved(std::path::Path::new(candidate)).is_ok_and(|p| p == self.profile)
                };
                argv.iter()
                    .any(|a| a.strip_prefix("--user-data-dir=").is_some_and(matches))
                    || argv
                        .windows(2)
                        .any(|a| a[0] == "--user-data-dir" && matches(a[1]))
            });
        if running || fs::symlink_metadata(self.profile.join("SingletonLock")).is_ok() {
            return Err(fail(
                "stop the VPN Chromium profile before importing its CA",
            ));
        }
        Ok(())
    }
}
