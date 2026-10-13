//! Timestamped, standalone SQL backups. Reuses the OSS-compatible S3 transport;
//! this does not enable cloud sync or replace its snapshots.

use std::io::Read;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::database::Database;
use crate::error::AppError;
use crate::settings::S3SyncSettings;

use super::s3::{self, S3Credentials};

const MAX_SQL_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OssBackupResult {
    pub oss_path: String,
    pub size_bytes: usize,
    pub sha256: String,
    pub uploaded_at: String,
}

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.to_string())
}

/// Accept regional or bucket-qualified OSS endpoints, always using HTTPS.
/// Checking the parsed hostname also prevents credentials going to lookalikes.
pub(crate) fn credentials(settings: &S3SyncSettings) -> Result<S3Credentials, AppError> {
    settings.validate()?;
    let bucket = settings.bucket.trim();
    let valid_edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !(3..=63).contains(&bucket.len())
        || !bucket.bytes().all(|b| valid_edge(b) || b == b'-')
        || !bucket.bytes().next().is_some_and(valid_edge)
        || !bucket.bytes().last().is_some_and(valid_edge)
    {
        return Err(invalid("OSS Bucket 名称无效"));
    }
    let endpoint = settings.endpoint.trim();
    let endpoint = if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("https://{endpoint}")
    };
    let url = Url::parse(&endpoint).map_err(|_| invalid("OSS Endpoint 无效"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("OSS Endpoint 必须是阿里云 OSS 的 HTTPS 地域地址"));
    }
    let host = url.host_str().ok_or_else(|| invalid("OSS Endpoint 无效"))?;
    let regional = host.strip_prefix(&format!("{bucket}.")).unwrap_or(host);
    let region = regional
        .strip_suffix(".aliyuncs.com")
        .or_else(|| regional.strip_suffix(".aliyuncs.com.cn"))
        .and_then(|name| name.strip_prefix("oss-"))
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .ok_or_else(|| invalid("OSS Endpoint 的 Bucket 或地域地址无效"))?;
    let access_key_id = settings.access_key_id.trim();
    if access_key_id.is_empty() || !access_key_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(invalid("请填写有效的 OSS AccessKey ID"));
    }
    if settings.secret_access_key.trim().is_empty()
        || settings.secret_access_key.chars().any(char::is_control)
    {
        return Err(invalid("请填写 OSS AccessKey Secret"));
    }
    let prefix = backup_prefix(settings);
    if prefix.contains(['?', '#', '\\'])
        || prefix.chars().any(char::is_control)
        || (!prefix.is_empty()
            && prefix
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(invalid("OSS 备份目录无效"));
    }
    Ok(S3Credentials {
        endpoint: format!("https://{regional}"),
        bucket: bucket.into(),
        region: region.strip_suffix("-internal").unwrap_or(region).into(),
        access_key_id: access_key_id.into(),
        secret_access_key: settings.secret_access_key.clone(),
    })
}

/// Read a selected SQL file without executing it, or export the current DB.
pub(crate) fn sql_bytes(db: &Database, file: Option<&Path>) -> Result<Vec<u8>, AppError> {
    let bytes = if let Some(path) = file {
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("sql"))
        {
            return Err(invalid("请选择 .sql 备份文件"));
        }
        let file = std::fs::File::open(path).map_err(|e| AppError::io(path, e))?;
        if !file
            .metadata()
            .map_err(|e| AppError::io(path, e))?
            .is_file()
        {
            return Err(invalid("请选择 SQL 文件"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_SQL_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| AppError::io(path, e))?;
        bytes
    } else {
        db.export_sql_string()?.into_bytes()
    };
    if bytes.is_empty() || bytes.len() > MAX_SQL_BYTES {
        return Err(invalid("SQL 备份文件须非空且不超过 128 MiB"));
    }
    std::str::from_utf8(&bytes).map_err(|_| invalid("SQL 文件须使用 UTF-8 编码"))?;
    Ok(bytes)
}

fn backup_prefix(settings: &S3SyncSettings) -> String {
    format!(
        "{}/sql-backups/{}",
        settings.remote_root.trim().trim_matches('/'),
        settings.profile.trim()
    )
}

fn object_key(prefix: &str) -> String {
    let filename = format!(
        "cc-switch-backup-{}-{}.sql",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        uuid::Uuid::new_v4()
    );
    let prefix = prefix.trim().trim_matches('/');
    if prefix.is_empty() {
        filename
    } else {
        format!("{prefix}/{filename}")
    }
}

pub(crate) async fn upload(
    settings: &S3SyncSettings,
    bytes: Vec<u8>,
) -> Result<OssBackupResult, AppError> {
    let creds = credentials(settings)?;
    let key = object_key(&backup_prefix(settings));
    upload_to(&creds, &key, bytes).await
}

async fn upload_to(
    creds: &S3Credentials,
    key: &str,
    bytes: Vec<u8>,
) -> Result<OssBackupResult, AppError> {
    let size_bytes = bytes.len();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    s3::put_private_object(creds, key, bytes, "application/sql; charset=utf-8").await?;
    let (remote, _) = s3::get_object(creds, key, size_bytes)
        .await?
        .ok_or_else(|| invalid("OSS 上传后未找到备份文件"))?;
    if remote.len() != size_bytes || format!("{:x}", Sha256::digest(&remote)) != sha256 {
        return Err(invalid("OSS 备份内容校验失败，请重新上传"));
    }
    Ok(OssBackupResult {
        oss_path: format!("oss://{}/{key}", creds.bucket),
        size_bytes,
        sha256,
        uploaded_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> S3SyncSettings {
        S3SyncSettings {
            endpoint: "oss-cn-hangzhou.aliyuncs.com".into(),
            bucket: "test-bucket".into(),
            access_key_id: "testKeyId".into(),
            secret_access_key: "testSecret".into(),
            region: "cn-hangzhou".into(),
            remote_root: "backups".into(),
            profile: "My-Mac".into(),
            ..Default::default()
        }
    }

    #[test]
    fn oss_backup_accepts_regional_and_matching_bucket_endpoints() {
        for endpoint in [
            "oss-cn-hangzhou.aliyuncs.com",
            "https://test-bucket.oss-cn-hangzhou.aliyuncs.com/",
        ] {
            let mut settings = config();
            settings.endpoint = endpoint.into();
            let creds = credentials(&settings).unwrap();
            assert_eq!(creds.region, "cn-hangzhou");
            assert_eq!(creds.endpoint, "https://oss-cn-hangzhou.aliyuncs.com");
        }
    }

    #[test]
    fn oss_backup_rejects_mismatched_hosts_and_unsafe_inputs() {
        for endpoint in [
            "http://oss-cn-hangzhou.aliyuncs.com",
            "https://other-bucket.oss-cn-hangzhou.aliyuncs.com",
            "https://oss-cn-hangzhou.aliyuncs.com.evil.test",
            "https://oss-cn-hangzhou.aliyuncs.com/path",
            "https://user:pass@oss-cn-hangzhou.aliyuncs.com",
        ] {
            let mut settings = config();
            settings.endpoint = endpoint.into();
            assert!(credentials(&settings).is_err(), "{endpoint}");
        }
        for prefix in ["../sql", "sql//backups", "sql?acl", "sql#fragment"] {
            let mut settings = config();
            settings.remote_root = prefix.into();
            assert!(credentials(&settings).is_err());
        }
    }

    #[test]
    fn oss_backup_exports_restorable_sql_and_reads_selected_file_without_importing() {
        let db = Database::memory().unwrap();
        let sql = sql_bytes(&db, None).unwrap();
        let exported = String::from_utf8(sql).unwrap();
        let restored = rusqlite::Connection::open_in_memory().unwrap();
        restored.execute_batch(&exported).unwrap();
        let count: i64 = restored
            .query_row("SELECT COUNT(*) FROM proxy_config", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5);
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("selected.sql");
        let sql = b"SELECT 'backup only; never execute';";
        std::fs::write(&file, sql).unwrap();
        assert_eq!(sql_bytes(&db, Some(&file)).unwrap(), sql);
        std::fs::write(&file, []).unwrap();
        assert!(sql_bytes(&db, Some(&file)).is_err());
        assert!(sql_bytes(&db, Some(&directory.path().join("backup.db"))).is_err());
    }

    #[test]
    fn oss_backup_keys_are_independent_timestamped_sql_files() {
        let first = object_key("/backups/sql/");
        assert!(first.starts_with("backups/sql/cc-switch-backup-"));
        assert!(first.ends_with(".sql"));
        assert_ne!(first, object_key("backups/sql"));
        assert!(!object_key("").starts_with('/'));
    }

    #[tokio::test]
    async fn oss_backup_uploads_private_sql_and_rejects_corrupt_readback() {
        use axum::{body::Bytes, extract::State, http::HeaderMap, routing::put, Router};
        use std::sync::{Arc, Mutex};

        for corrupt in [false, true] {
            let stored = Arc::new(Mutex::new((Vec::<u8>::new(), false)));
            let app = Router::new()
                .route(
                    "/test-bucket/backup.sql",
                    put(
                        |State(stored): State<Arc<Mutex<(Vec<u8>, bool)>>>,
                         headers: HeaderMap,
                         bytes: Bytes| async move {
                            let private = headers.get("x-amz-acl").is_some_and(|h| h == "private")
                                && headers
                                    .get("authorization")
                                    .is_some_and(|h| h.to_str().unwrap().contains("x-amz-acl"));
                            *stored.lock().unwrap() = (bytes.to_vec(), private);
                        },
                    )
                    .get(
                        move |State(stored): State<Arc<Mutex<(Vec<u8>, bool)>>>| async move {
                            if corrupt {
                                b"corrupt".to_vec()
                            } else {
                                stored.lock().unwrap().0.clone()
                            }
                        },
                    ),
                )
                .with_state(stored.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let creds = S3Credentials {
                endpoint: format!("http://{address}"),
                bucket: "test-bucket".into(),
                region: "cn-hangzhou".into(),
                access_key_id: "testId".into(),
                secret_access_key: "testSecret".into(),
            };
            let sql = b"SELECT 'SQL backup';".to_vec();
            let result = upload_to(&creds, "backup.sql", sql.clone()).await;
            assert_eq!(result.is_err(), corrupt);
            assert_eq!(stored.lock().unwrap().0, sql);
            assert!(
                stored.lock().unwrap().1,
                "the private ACL must be signed and sent"
            );
            server.abort();
        }
    }

    #[tokio::test]
    #[ignore = "requires OSS_ACCESS_KEY_ID/OSS_ACCESS_KEY_SECRET and an authorized test bucket"]
    async fn oss_backup_real_cloud_smoke() {
        let mut settings = config();
        settings.bucket = std::env::var("OSS_BUCKET").expect("OSS_BUCKET");
        settings.endpoint = std::env::var("OSS_ENDPOINT").expect("OSS_ENDPOINT");
        settings.access_key_id = std::env::var("OSS_ACCESS_KEY_ID").expect("OSS_ACCESS_KEY_ID");
        settings.secret_access_key =
            std::env::var("OSS_ACCESS_KEY_SECRET").expect("OSS_ACCESS_KEY_SECRET");
        settings.remote_root = "cc-switch-tests".into();
        settings.profile = crate::settings::default_sync_profile();
        let result = upload(
            &settings,
            b"-- CC Switch OSS upload smoke test\nSELECT 1;\n".to_vec(),
        )
        .await
        .unwrap();
        let creds = credentials(&settings).unwrap();
        let key = result
            .oss_path
            .strip_prefix(&format!("oss://{}/", settings.bucket))
            .unwrap();
        let public_url = format!(
            "https://{}.{}/{}",
            settings.bucket,
            creds.endpoint.trim_start_matches("https://"),
            key
        );
        let status = reqwest::Client::new()
            .get(public_url)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(
            status,
            reqwest::StatusCode::FORBIDDEN,
            "SQL backups must not allow anonymous access"
        );
        println!("Verified private SQL backup: {}", result.oss_path);
    }
}
