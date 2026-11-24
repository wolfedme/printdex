//! Opening a data folder whose database is newer, damaged or unwritable.

use std::fs;
use std::path::Path;
use std::str::FromStr;

use printdex_core::{CoreError, Engine};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

fn hash(path: &Path) -> u64 {
    xxhash_rust::xxh3::xxh3_64(&fs::read(path).unwrap())
}

/// Opens and closes an engine so `data/printdex.db` exists with nothing
/// left in its `-wal`.
async fn created(data: &Path) {
    let engine = Engine::open(data).await.unwrap();
    engine.close().await;
}

#[tokio::test]
async fn newer_schema_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    created(&data).await;
    let db = data.join("printdex.db");
    {
        let opts = SqliteConnectOptions::from_str("sqlite://")
            .unwrap()
            .filename(&db);
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, success, checksum, execution_time) \
             VALUES (9999, 'from the future', 1, x'00', 0)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
    }
    let before = hash(&db);
    let r = Engine::open(&data).await;
    assert!(
        matches!(&r, Err(CoreError::DataTooNew(p)) if p == &db),
        "{:?}",
        r.err()
    );
    assert_eq!(hash(&db), before, "the newer database is left as it was");
}

#[tokio::test]
async fn corrupt_db_is_reported_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = data.join("printdex.db");
    fs::write(&db, [0xAB; 4096]).unwrap();
    let r = Engine::open(&data).await;
    assert!(
        matches!(&r, Err(CoreError::DataCorrupt(p)) if p == &db),
        "{:?}",
        r.err()
    );
    assert_eq!(fs::read(&db).unwrap(), [0xAB; 4096]);
}

/// A database damaged past its header: opening passes the header check
/// but a page further in is garbage, which `quick_check` finds.
#[tokio::test]
async fn damaged_pages_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    created(&data).await;
    let db = data.join("printdex.db");
    let mut bytes = fs::read(&db).unwrap();
    // Page 1 holds the header and the schema root; ruin everything after.
    for b in &mut bytes[4096..] {
        *b = 0xAB;
    }
    fs::write(&db, &bytes).unwrap();
    let r = Engine::open(&data).await;
    assert!(
        matches!(&r, Err(CoreError::DataCorrupt(p)) if p == &db),
        "{:?}",
        r.err()
    );
    assert_eq!(fs::read(&db).unwrap(), bytes);
}

#[cfg(unix)]
#[tokio::test]
async fn read_only_data_folder_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        // Root writes through any permission bits.
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    created(&data).await;
    let db = data.join("printdex.db");
    fs::set_permissions(&db, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o555)).unwrap();
    let r = Engine::open(&data).await;
    // Writable again, so the temporary folder can be removed.
    fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&db, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        matches!(&r, Err(CoreError::DataReadOnly(p)) if p == &db),
        "{:?}",
        r.err()
    );
}

#[tokio::test]
async fn set_aside_keeps_old_file() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = data.join("printdex.db");
    fs::write(&db, [0xAB; 4096]).unwrap();
    assert!(matches!(
        Engine::open(&data).await,
        Err(CoreError::DataCorrupt(_))
    ));
    // Written after the failed open, which may tidy away a `-wal` it
    // can't use.
    fs::write(data.join("printdex.db-wal"), b"old wal").unwrap();

    let aside = Engine::set_aside_database(&data).unwrap();
    assert_eq!(aside.parent(), Some(data.as_path()));
    let name = aside.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("printdex.db.broken-"), "{name}");
    assert_eq!(fs::read(&aside).unwrap(), [0xAB; 4096]);
    assert!(!db.exists());
    let wal = data.join(format!("{name}-wal"));
    assert_eq!(fs::read(wal).unwrap(), b"old wal");
    assert!(!data.join("printdex.db-wal").exists());

    let engine = Engine::open(&data).await.unwrap();
    assert!(engine.libraries().await.unwrap().is_empty());
    assert!(engine.library_counts().await.unwrap().is_empty());
}

#[tokio::test]
async fn set_aside_without_database_fails() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Engine::set_aside_database(dir.path()).is_err());
}
